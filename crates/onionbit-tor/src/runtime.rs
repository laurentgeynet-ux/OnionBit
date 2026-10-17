// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runtime Tor : cycle de vie du client et fetch borne.
//!
//! Invariants (ADR-0028) :
//!
//! - **Bootstrap explicite et jamais bloquant** : [`TorRuntime::bootstrap`]
//!   ne fait que lancer une tache de fond — un FAI filtrant ne gele
//!   jamais le daemon ; l'etat reste observable via
//!   [`TorRuntime::state`] (`/api/tor/status`).
//! - **Frontiere dure** : `fetch_onion` passe chaque cible — y compris
//!   chaque redirection — par `TorTargetPolicy::check` **avant**
//!   d'ouvrir la moindre connexion.
//! - **Pas de repli clearnet** : indisponible → erreur explicite.
//! - Le transport reel vit derriere la feature `tor` : sans elle,
//!   bootstrap et fetch rendent [`TorError::NotBuilt`].

use std::sync::{Arc, Mutex};

use crate::error::{Result, TorError};
#[cfg(feature = "tor")]
use crate::http;
use crate::http::OnionResponse;
#[cfg(feature = "tor")]
use crate::settings::TorMode;
use crate::settings::TorSettings;

/// Etat observable du runtime (`GET /api/tor/status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorState {
    /// Tor desactive (`tor.enabled = false`) ou runtime non cree.
    Off,
    /// Bootstrap en cours (telechargement du consensus, gardes).
    Bootstrapping {
        /// Progression arti 0-100 quand connue.
        progress: u8,
    },
    /// Circuits utilisables.
    Ready,
    /// Client dormant — reserve : arti gere la dormance lui-meme
    /// entre usages (pas d'API d'etat directe) ; l'etat est expose
    /// pour l'API `/api/tor/status` et une future remontee fine.
    Dormant,
    /// Bootstrap ou transport en echec — daemon operationnel, la
    /// recherche externe rend une erreur explicite.
    Error(String),
}

/// Flux TCP anonymise (DataStream arti ou SOCKS5 externe).
#[cfg(feature = "tor")]
pub trait OnionIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
#[cfg(feature = "tor")]
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> OnionIo for T {}

/// Consommateur d'un flux accepte sur le service `.onion` d'identite
/// (ADR-0029 etape 124) — un handler par port virtuel `BEGIN`. Le
/// proprietaire du protocole (messagerie, catalogue, coffre)
/// enregistre le sien ; `tor` ne fait que router, jamais traduire.
/// Le flux passe sous `Box<dyn OnionIo>` : aucun type arti ne fuit
/// vers les crates consommatrices.
#[cfg(feature = "hs")]
pub trait OnionLinkHandler: Send + Sync {
    /// Appelle pour chaque `StreamRequest` acceptee sur le port
    /// souscrit — l'implementeur traite le flux dans sa propre
    /// tache (cette methode doit rendre la main vite).
    fn on_stream(&self, stream: Box<dyn OnionIo>);
}

/// Etat partage du runtime — clonable (Arc) : le daemon garde un
/// handle pour l'API pendant que la tache de bootstrap mute l'etat.
pub(crate) struct Inner {
    pub(crate) settings: TorSettings,
    /// Allowlist etendue aux pairs approuves (ADR-0029) — mutable a
    /// chaud : les `.onion` associes explicitement aux
    /// contacts/canaux s'y enregistrent ; la politique de config
    /// (`settings.target_policy`, recherche) reste immuable.
    pub(crate) peer_policy: Mutex<onionbit_network_policy::TorTargetPolicy>,
    state: Mutex<TorState>,
    /// Tache de bootstrap en cours — idempotence + re-essai.
    #[cfg(feature = "tor")]
    boot: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Client arti bootstrappe (mode `embedded`).
    #[cfg(feature = "tor")]
    pub(crate) client: Mutex<Option<Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>>>,
    /// Service `.onion` d'identite heberge (ADR-0029) — vivant tant
    /// que le slot existe ; remplace ou droppe = depublie.
    #[cfg(feature = "hs")]
    pub(crate) onion: Mutex<Option<crate::service::OnionServiceSlot>>,
    /// Handlers par port virtuel `BEGIN` — consultes a chaque
    /// `StreamRequest` : une injection tardive (messagerie prete
    /// apres le lancement du service) sert sans republication.
    /// `None`/port absent = rejet `DONE` (le descripteur ne
    /// promet aucun service non routable).
    #[cfg(feature = "hs")]
    pub(crate) link_handlers: Mutex<std::collections::HashMap<u16, Arc<dyn OnionLinkHandler>>>,
}

/// Runtime Tor — proprietaire unique de tout trafic Tor du daemon.
#[derive(Clone)]
pub struct TorRuntime {
    inner: Arc<Inner>,
}

fn set_state(inner: &Inner, state: TorState) {
    *inner.state.lock().unwrap_or_else(|e| e.into_inner()) = state;
}

/// Vue publique du service `.onion` d'identite — remonte a
/// `GET /api/tor/status`.
#[derive(Debug, Clone)]
pub struct OnionServiceInfo {
    /// `tor.onion_service.enabled` (ou service reellement lance).
    pub enabled: bool,
    /// `off` | `bootstrapping` | `running` | `degraded` |
    /// `shutdown` — etat haut niveau remonte par arti.
    pub state: String,
    /// Adresse `.onion` hebergee — `Some` uniquement quand le
    /// service tourne.
    pub address: Option<String>,
}

impl TorRuntime {
    /// Cree le runtime sans bootstrap — `Off` tant que
    /// [`TorRuntime::bootstrap`] n'a pas ete appele (bootstrap
    /// explicite, jamais au `new` ni au demarrage du daemon).
    pub fn new(settings: TorSettings) -> Self {
        Self {
            inner: Arc::new(Inner {
                settings,
                peer_policy: Mutex::new(onionbit_network_policy::TorTargetPolicy::new()),
                state: Mutex::new(TorState::Off),
                #[cfg(feature = "tor")]
                boot: Mutex::new(None),
                #[cfg(feature = "tor")]
                client: Mutex::new(None),
                #[cfg(feature = "hs")]
                onion: Mutex::new(None),
                #[cfg(feature = "hs")]
                link_handlers: Mutex::new(std::collections::HashMap::new()),
            }),
        }
    }

    /// Runtime factice a l'etat impose — reserve aux tests des
    /// couches superieures (API) qui injectent leurs propres
    /// backends : l'etat affiche `state` mais `fetch_onion` reste
    /// impossible (transport absent) — aucun octet ne sort jamais
    /// par un stub.
    pub fn stub_state(settings: TorSettings, state: TorState) -> Self {
        let rt = Self::new(settings);
        set_state(&rt.inner, state);
        rt
    }

    /// [`Self::stub_state`] a l'etat `Ready`.
    pub fn stub_ready(settings: TorSettings) -> Self {
        Self::stub_state(settings, TorState::Ready)
    }

    /// Deux handles partagent-ils le meme runtime ? (Clone =
    /// `Arc` partage.) Reserve aux tests verifiant qu'un hot-swap a
    /// — ou n'a pas — permute l'instance.
    #[doc(hidden)]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Reglages actifs.
    pub fn settings(&self) -> &TorSettings {
        &self.inner.settings
    }

    /// Etat courant (diagnostic `/api/tor/status`).
    pub fn state(&self) -> TorState {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Lance le bootstrap en tache de fond — retourne
    /// immediatement. Idempotent tant qu'un bootstrap tourne ;
    /// un bootstrap termine en erreur peut etre retente.
    ///
    /// - `embedded` : `TorClient` arti (`Manual` — rien ne sort avant
    ///   l'appel explicite), consensus/cache sous `state_dir/tor`,
    ///   progression via `bootstrap_events`.
    /// - `external` : sonde TCP du SOCKS5 loopback (le handshake
    ///   complet a lieu a chaque fetch).
    pub fn bootstrap(&self) -> Result<()> {
        if !self.inner.settings.enabled {
            return Err(TorError::Disabled);
        }
        #[cfg(feature = "tor")]
        {
            self.bootstrap_inner()
        }
        #[cfg(not(feature = "tor"))]
        {
            set_state(
                &self.inner,
                TorState::Error("feature `tor` desactivee".into()),
            );
            Err(TorError::NotBuilt)
        }
    }

    #[cfg(feature = "tor")]
    fn bootstrap_inner(&self) -> Result<()> {
        let mut boot = self.inner.boot.lock().unwrap_or_else(|e| e.into_inner());
        if boot.as_ref().is_some_and(|h| !h.is_finished()) {
            return Ok(());
        }
        set_state(&self.inner, TorState::Bootstrapping { progress: 0 });
        let inner = self.inner.clone();
        *boot = Some(tokio::spawn(async move {
            match inner.settings.mode {
                TorMode::External(addr) => bootstrap_external(&inner, addr).await,
                TorMode::Embedded => bootstrap_embedded(&inner).await,
            }
        }));
        Ok(())
    }

    /// Fetch borne d'une ressource `.onion` (`GET`, reponse ≤
    /// `max_response_bytes`). Chaque cible — dont chaque hop de
    /// redirection — passe `TorTargetPolicy::check` avant connexion ;
    /// aucune URL clearnet ni onion non declare n'est jamais jointe.
    pub async fn fetch_onion(&self, url: &str) -> Result<OnionResponse> {
        if !self.inner.settings.enabled {
            return Err(TorError::Disabled);
        }
        #[cfg(feature = "tor")]
        {
            self.fetch_bounded(url).await
        }
        #[cfg(not(feature = "tor"))]
        {
            let _ = url;
            Err(TorError::NotBuilt)
        }
    }

    /// Boucle de fetch avec redirections bornees (`max_redirects`,
    /// uniquement vers un hote qui repassera la politique).
    #[cfg(feature = "tor")]
    async fn fetch_bounded(&self, url: &str) -> Result<OnionResponse> {
        let settings = &self.inner.settings;
        let mut current = url.to_string();
        for _ in 0..=settings.max_redirects {
            let target = http::parse_target(&current)?;
            // Politique effective = cibles de config ∪ pairs
            // approuves (catalogues `.onion` des contacts/canaux,
            // ADR-0029 etape 126).
            if settings
                .target_policy
                .check(&target.host, target.port)
                .is_err()
            {
                self.inner
                    .peer_policy
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .check(&target.host, target.port)?;
            }
            let stream = self.open_stream(&target.host, target.port).await?;
            // `https` : TLS au-dessus du flux Tor — licite pour
            // les services onion dument certifies (le transport
            // reste end-to-end chiffre et authentifie par le
            // rendez-vous onion).
            let stream = if target.tls {
                http::wrap_tls(stream, &target.host).await?
            } else {
                stream
            };
            let request = http::build_request(&target);
            let resp = http::exchange(
                stream,
                &request,
                settings.max_response_bytes,
                settings.request_timeout,
            )
            .await?;
            if resp.is_redirect() {
                current = http::resolve_redirect(&current, &resp)?;
                continue;
            }
            return Ok(resp);
        }
        Err(TorError::RedirectDenied("max_redirects depasse".into()))
    }

    /// Lance le service `.onion` d'identite (ADR-0029 etape 122)
    /// sur la lane embedded — `hs_seed` est le materiel ed25519
    /// derive HKDF `onionbit/identity/tor-hs/v1`, jamais `ipv8-sign`.
    /// Idempotent : un relancement remplace le slot (depublie
    /// l'ancien) — l'adresse rederivee est identique.
    ///
    /// Erreurs explicites : `Disabled` (`tor.enabled=false`),
    /// `NotReady` (client pas encore bootstrappe), `OnionService`
    /// (mode externe, keystore ou lancement en echec, feature `hs`
    /// non compilee).
    pub fn launch_onion_identity(&self, hs_seed: [u8; 32]) -> Result<()> {
        if !self.inner.settings.enabled {
            return Err(TorError::Disabled);
        }
        if !self.inner.settings.onion_service.enabled {
            return Err(TorError::OnionService(
                "service .onion desactive (tor.onion_service.enabled=false)".into(),
            ));
        }
        #[cfg(feature = "hs")]
        {
            crate::service::launch(&self.inner, hs_seed)
        }
        #[cfg(not(feature = "hs"))]
        {
            let _ = hs_seed;
            Err(TorError::OnionService(
                "service .onion non compile (feature `hs` desactivee)".into(),
            ))
        }
    }

    /// ADR-0029 — enregistre le `.onion` d'un pair approuve dans
    /// l'allowlist etendue (association explicite contact/canal).
    /// La politique effective d'un fetch devient
    /// `target_policy ∪ peer_policy` — jamais l'inverse : les pairs
    /// n'elargissent pas la recherche, la recherche n'autorise pas
    /// de pair arbitraire. `Err` si l'entree n'est pas un v3 valide.
    pub fn allow_peer_onion(&self, host: &str) -> Result<()> {
        self.inner
            .peer_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(host)?;
        Ok(())
    }

    /// Revoque le `.onion` d'un pair (association oubliee).
    /// `true` si l'entree existait.
    pub fn revoke_peer_onion(&self, host: &str) -> bool {
        self.inner
            .peer_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(host)
    }

    /// `true` si `host` est un `.onion` de pair approuve.
    pub fn peer_onion_allowed(&self, host: &str) -> bool {
        self.inner
            .peer_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_allowed(host)
    }

    /// Ouvre un flux TCP anonymise vers le `.onion` d'un **pair
    /// approuve** (ADR-0029 etape 124) — le lien messagerie, et plus
    /// tard le catalogue/coffre, passent par ce seul point.
    ///
    /// Frontieres : host `.onion` v3 valide ET dans l'allowlist
    /// **pairs** (`peer_policy` seule — les cibles de recherche
    /// n'autorisent jamais une connexion applicative, et le port
    /// virtuel n'est pas borne a 80/443 : il sert les services
    /// OnionBit). `DeniedTarget` sinon ; `NotReady` sans client.
    /// `request_timeout` borne l'etablissement.
    #[cfg(feature = "tor")]
    pub async fn connect_onion(&self, host: &str, port: u16) -> Result<Box<dyn OnionIo>> {
        let addr = onionbit_format::OnionAddr::parse(host).map_err(|_| {
            TorError::DeniedTarget(onionbit_network_policy::PolicyError::DeniedDestination(
                "adresse .onion de pair invalide",
            ))
        })?;
        if !self
            .inner
            .peer_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_allowed(addr.host())
        {
            return Err(TorError::DeniedTarget(
                onionbit_network_policy::PolicyError::DeniedDestination("pair .onion non approuve"),
            ));
        }
        tokio::time::timeout(
            self.inner.settings.request_timeout,
            self.open_stream(addr.host(), port),
        )
        .await
        .map_err(|_| {
            TorError::Request(format!(
                "connexion .onion expiree ({}s)",
                self.inner.settings.request_timeout.as_secs()
            ))
        })?
    }

    /// Souscrit `handler` au port virtuel `port` du service
    /// `.onion` d'identite — mutable a chaud : appelable avant ou
    /// apres le lancement (les `StreamRequest` sont routes a la
    /// reception, jamais memorisees). Remplace le precedent.
    #[cfg(feature = "hs")]
    pub fn set_onion_link_handler(&self, port: u16, handler: Arc<dyn OnionLinkHandler>) {
        self.inner
            .link_handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(port, handler);
    }

    /// Etat du service `.onion` d'identite pour
    /// `GET /api/tor/status` — `address` n'est renseignee que si le
    /// service est reellement lance : jamais de joignabilite fictive
    /// (ADR-0028 §8).
    pub fn onion_service(&self) -> OnionServiceInfo {
        #[cfg(feature = "hs")]
        {
            let guard = self.inner.onion.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(slot) = guard.as_ref() {
                return OnionServiceInfo {
                    enabled: true,
                    state: (*slot.state.lock().unwrap_or_else(|e| e.into_inner())).to_string(),
                    address: slot.address.clone(),
                };
            }
        }
        OnionServiceInfo {
            enabled: self.inner.settings.onion_service.enabled,
            state: "off".into(),
            address: None,
        }
    }

    /// Ouvre un flux TCP anonymise vers `(host, port)` deja valide
    /// par la politique. Jamais de `connect` expose hors de ce
    /// chemin — le fetcher n'est pas un proxy generique.
    #[cfg(feature = "tor")]
    async fn open_stream(&self, host: &str, port: u16) -> Result<Box<dyn OnionIo>> {
        match self.inner.settings.mode {
            TorMode::Embedded => {
                let client = self
                    .inner
                    .client
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .ok_or(TorError::NotReady)?;
                let stream = client
                    .connect((host, port))
                    .await
                    .map_err(|e| TorError::Request(e.to_string()))?;
                Ok(Box::new(stream))
            }
            TorMode::External(addr) => {
                let stream = tokio_socks::tcp::Socks5Stream::connect(addr, (host, port))
                    .await
                    .map_err(|e| TorError::Request(e.to_string()))?;
                Ok(Box::new(stream))
            }
        }
    }
}

/// Sonde le SOCKS5 loopback d'un Tor systeme : TCP connect suffit —
/// le handshake complet (et l'eventuelle erreur distante) a lieu a
/// chaque fetch.
#[cfg(feature = "tor")]
async fn bootstrap_external(inner: &Arc<Inner>, addr: std::net::SocketAddr) {
    if !inner.settings.bridges.is_empty() {
        // Les ponts sont une config du client embarque : en mode
        // externe ils relevent du tor systeme (torrc) — jamais
        // ignores silencieusement.
        tracing::warn!(
            "tor.bridges configure mais mode=external — les ponts sont geres par le tor systeme (torrc)"
        );
    }
    let probe = tokio::net::TcpStream::connect(addr);
    match tokio::time::timeout(inner.settings.bootstrap_timeout, probe).await {
        Ok(Ok(_)) => set_state(inner, TorState::Ready),
        Ok(Err(e)) => set_state(
            inner,
            TorState::Error(format!("SOCKS5 externe injoignable : {e}")),
        ),
        Err(_) => set_state(inner, TorState::Error("sonde SOCKS5 expiree".into())),
    }
}

/// Parse les lignes de pont Tor : chaque ligne valide rend un
/// builder pret a pousser ; chaque ligne invalide (ou transport PT
/// non compile — `pt-client` absent) est loggee et ignoree.
#[cfg(feature = "tor")]
fn ponts_valides(lignes: &[String]) -> Vec<arti_client::config::BridgeConfigBuilder> {
    use arti_client::config::BridgeConfigBuilder;
    lignes
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| match l.parse::<BridgeConfigBuilder>() {
            Ok(b) => Some(b),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "ligne de pont Tor invalide ou transport PT non compile — ignoree"
                );
                None
            }
        })
        .collect()
}

/// Bootstrap arti : `BootstrapBehavior::Manual` (aucune connexion
/// reseau avant cet appel explicite), etat/cache sous
/// `state_dir/tor/{state,cache}`, progression remontee via
/// `bootstrap_events`. Jamais appele au demarrage du daemon.
#[cfg(feature = "tor")]
async fn bootstrap_embedded(inner: &Arc<Inner>) {
    use arti_client::config::TorClientConfigBuilder;
    use arti_client::{BootstrapBehavior, TorClient};

    let settings = &inner.settings;
    if settings.state_dir.as_os_str().is_empty() {
        set_state(
            inner,
            TorState::Error("state_dir tor absent — embedded impossible".into()),
        );
        return;
    }
    let outcome = async {
        let tor_dir = settings.state_dir.clone();
        std::fs::create_dir_all(&tor_dir)
            .map_err(|e| TorError::BootstrapFailed(format!("state_dir : {e}")))?;
        // Etat/consensus/gardes sous `state_dir/tor/{state,cache}` —
        // `from_directories` derive aussi le keystore du state dir.
        let mut builder =
            TorClientConfigBuilder::from_directories(tor_dir.join("state"), tor_dir.join("cache"));
        // Ponts Tor (anti-censure IP — `tor/bridges`) : relays non
        // listés à la place des gardes publics. Sans `pt-client`, les
        // lignes a transport (`obfs4`…) echouent au parse — warn +
        // skip. Si AUCUN pont configure n'est valide → erreur nette :
        // bootstrapper quand meme sur les gardes publics
        // contournerait silencieusement l'intention « ponts
        // uniquement » (l'utilisateur croirait le pont actif alors
        // que la lane passe a decouvert).
        if !settings.bridges.is_empty() {
            use arti_client::config::BoolOrAuto;
            let parsed = ponts_valides(&settings.bridges);
            if parsed.is_empty() {
                Err(TorError::BootstrapFailed(
                    "aucune ligne de pont Tor valide".into(),
                ))?;
            }
            let count = parsed.len();
            for b in parsed {
                builder.bridges().bridges().push(b);
            }
            builder.bridges().enabled(BoolOrAuto::Explicit(true));
            tracing::info!(count, "ponts Tor configures — gardes publics exclus");
        }
        let config = builder
            .build()
            .map_err(|e| TorError::BootstrapFailed(e.to_string()))?;
        let client = TorClient::builder()
            .config(config)
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped_async()
            .await
            .map_err(|e| TorError::BootstrapFailed(e.to_string()))?;
        // Progression → etat observable (l'UI lit `progress`).
        let mut events = client.bootstrap_events();
        let progress_inner = inner.clone();
        tokio::spawn(async move {
            use futures_util::StreamExt;
            while let Some(status) = events.next().await {
                let mut st = progress_inner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if matches!(*st, TorState::Bootstrapping { .. }) {
                    *st = TorState::Bootstrapping {
                        progress: (status.as_frac() * 100.0) as u8,
                    };
                }
            }
        });
        tokio::time::timeout(settings.bootstrap_timeout, client.bootstrap())
            .await
            .map_err(|_| TorError::BootstrapFailed("bootstrap_timeout".into()))?
            .map_err(|e| TorError::BootstrapFailed(e.to_string()))?;
        Ok::<_, TorError>(client)
    }
    .await;
    match outcome {
        Ok(client) => {
            *inner.client.lock().unwrap_or_else(|e| e.into_inner()) = Some(client);
            set_state(inner, TorState::Ready);
        }
        Err(e) => set_state(inner, TorState::Error(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_network_policy::TorTargetPolicy;
    use std::time::Duration;

    const TPB: &str = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";

    fn settings_with_policy() -> TorSettings {
        let mut p = TorTargetPolicy::new();
        p.insert(TPB).unwrap();
        TorSettings {
            enabled: true,
            target_policy: p,
            request_timeout: Duration::from_secs(5),
            ..TorSettings::default()
        }
    }

    #[tokio::test]
    async fn runtime_desactive_refuse_sans_fuite() {
        let rt = TorRuntime::new(TorSettings::default());
        assert_eq!(rt.state(), TorState::Off);
        assert!(matches!(
            rt.fetch_onion("http://x.onion").await,
            Err(TorError::Disabled)
        ));
        assert!(matches!(rt.bootstrap(), Err(TorError::Disabled)));
        assert_eq!(rt.state(), TorState::Off);
    }

    /// Service `.onion` : sans activation le lancement est refuse ;
    /// sans la feature `hs` l'erreur est explicite et l'etat expose
    /// reste `off` — aucune adresse fictive.
    #[tokio::test]
    async fn service_onion_refuse_sans_activation() {
        let rt = TorRuntime::new(TorSettings {
            enabled: true,
            ..TorSettings::default()
        });
        let err = rt.launch_onion_identity([7u8; 32]).unwrap_err();
        assert!(matches!(err, TorError::OnionService(_)));
        let info = rt.onion_service();
        assert!(!info.enabled);
        assert_eq!(info.state, "off");
        assert!(info.address.is_none());

        // Desactive globalement → Disabled prime.
        let rt2 = TorRuntime::new(TorSettings::default());
        assert!(matches!(
            rt2.launch_onion_identity([7u8; 32]),
            Err(TorError::Disabled)
        ));
    }

    /// Sans la feature `tor`, le bootstrap rend une erreur explicite
    /// — aucun trafic n'est tente.
    #[cfg(not(feature = "tor"))]
    #[tokio::test]
    async fn bootstrap_sans_transport_erreur_explicite() {
        let rt = TorRuntime::new(settings_with_policy());
        let err = rt.bootstrap().unwrap_err();
        assert!(matches!(err, TorError::NotBuilt));
        assert!(matches!(rt.state(), TorState::Error(_)));
    }

    /// Mini SOCKS5 + HTTP : accepte le handshake, puis sert
    /// `respond` pour chaque connexion recue (une par fetch/redirect).
    #[cfg(feature = "tor")]
    async fn fake_socks5(
        responses: Vec<Vec<u8>>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<usize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            /// Une connexion : handshake SOCKS5 + lecture requete +
            /// reponse. Les connexions mortes (sonde bootstrap)
            /// rendent Err et sont ignorees.
            async fn handle(s: &mut tokio::net::TcpStream, resp: &[u8]) -> std::io::Result<()> {
                // Greeting : VER=5 NMETHODS n METHODS…
                let mut head = [0u8; 2];
                s.read_exact(&mut head).await?;
                let mut methods = vec![0u8; head[1] as usize];
                s.read_exact(&mut methods).await?;
                s.write_all(&[0x05, 0x00]).await?;
                // CONNECT : VER CMD RSV ATYP … PORT
                let mut req = [0u8; 4];
                s.read_exact(&mut req).await?;
                let need = match req[3] {
                    1 => 6, // IPv4 + port
                    3 => {
                        let mut l = [0u8; 1];
                        s.read_exact(&mut l).await?;
                        l[0] as usize + 2
                    }
                    4 => 18, // IPv6 + port
                    _ => 0,
                };
                let mut rest = vec![0u8; need];
                s.read_exact(&mut rest).await?;
                // Succes : VER REP RSV ATYP=BND.ADDR(0.0.0.0) PORT=0
                s.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await?;
                // Lit la requete HTTP (jusqu'a \r\n\r\n) puis sert.
                let mut buf = Vec::new();
                let mut b = [0u8; 1024];
                loop {
                    let n = s.read(&mut b).await?;
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&b[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                s.write_all(resp).await?;
                s.shutdown().await
            }
            let mut served = 0usize;
            for resp in responses {
                loop {
                    let Ok((mut s, _)) = listener.accept().await else {
                        return served;
                    };
                    if handle(&mut s, &resp).await.is_ok() {
                        served += 1;
                        break;
                    }
                }
            }
            served
        });
        (addr, task)
    }

    #[cfg(feature = "tor")]
    fn external_settings(proxy: std::net::SocketAddr) -> TorSettings {
        TorSettings {
            mode: TorMode::External(proxy),
            ..settings_with_policy()
        }
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn fetch_external_sert_la_reponse() {
        let body = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec();
        let (addr, server) = fake_socks5(vec![body]).await;
        let rt = TorRuntime::new(external_settings(addr));
        rt.bootstrap().unwrap();
        // La sonde marque Ready.
        for _ in 0..50 {
            if rt.state() == TorState::Ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(rt.state(), TorState::Ready);
        let r = rt
            .fetch_onion(&format!("http://{TPB}/search"))
            .await
            .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"ok");
        assert_eq!(server.await.unwrap(), 1);
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn cible_hors_allowlist_refusee_avant_connexion() {
        let (addr, server) = fake_socks5(vec![]).await;
        let rt = TorRuntime::new(external_settings(addr));
        // Aucune socket ne doit etre ouverte : la politique coupe avant.
        assert!(matches!(
            rt.fetch_onion("http://thepiratebay.org/").await,
            Err(TorError::DeniedTarget(_))
        ));
        assert!(matches!(
            rt.fetch_onion(
                "http://juhanurmihxlp77nkq76byazcldy2hlmovfu2epvl5ankdibsot4csyd.onion/"
            )
            .await,
            Err(TorError::DeniedTarget(_)) // v3 valide mais non declare
        ));
        assert!(matches!(
            rt.fetch_onion(&format!("http://{TPB}:8080/")).await,
            Err(TorError::DeniedTarget(_)) // port hors politique
        ));
        server.abort();
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn redirection_suivie_puis_bornee() {
        let redirect =
            b"HTTP/1.1 302 Found\r\nLocation: /dest\r\nContent-Length: 0\r\n\r\n".to_vec();
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndone".to_vec();
        let (addr, server) = fake_socks5(vec![redirect, ok]).await;
        let rt = TorRuntime::new(external_settings(addr));
        let r = rt
            .fetch_onion(&format!("http://{TPB}/start"))
            .await
            .unwrap();
        assert_eq!(r.body, b"done");
        assert_eq!(server.await.unwrap(), 2);

        // Chaque hop repond 302 → max_redirects depasse.
        let always_redirect =
            vec![b"HTTP/1.1 302 Found\r\nLocation: /a\r\nContent-Length: 0\r\n\r\n".to_vec(); 5];
        let (addr2, server2) = fake_socks5(always_redirect).await;
        let rt2 = TorRuntime::new(external_settings(addr2));
        assert!(matches!(
            rt2.fetch_onion(&format!("http://{TPB}/")).await,
            Err(TorError::RedirectDenied(_))
        ));
        server2.abort();
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn redirection_hors_allowlist_refusee() {
        let evil =
            b"HTTP/1.1 301 Moved\r\nLocation: http://clearnet.example/\r\nContent-Length: 0\r\n\r\n"
                .to_vec();
        let (addr, _server) = fake_socks5(vec![evil]).await;
        let rt = TorRuntime::new(external_settings(addr));
        // La cible clearnet de la redirection est refusee par la
        // politique avant toute seconde connexion.
        assert!(matches!(
            rt.fetch_onion(&format!("http://{TPB}/")).await,
            Err(TorError::DeniedTarget(_))
                | Err(TorError::RedirectDenied(_))
                | Err(TorError::InvalidUrl(_))
        ));
    }

    /// `ponts_valides` : lignes correctes conservees, vides ignorees,
    /// invalides sautees sans panique.
    #[cfg(feature = "tor")]
    #[test]
    fn ponts_valides_filtre_et_survit() {
        // Ligne canonique arti : `Bridge <addr>:<port> <empreinte>`.
        let ok = "Bridge 192.0.2.55:9001 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let lignes = vec![
            String::new(),
            "   ".to_string(),
            ok.to_string(),
            "pas-une-ligne-de-pont".to_string(),
            "garbage garbage garbage".to_string(),
        ];
        let parsed = ponts_valides(&lignes);
        assert_eq!(parsed.len(), 1);
        // Liste vide → rien, jamais de panique.
        assert!(ponts_valides(&[]).is_empty());
    }

    /// Un transport PT (`obfs4`…) n'est pas compilable sans
    /// `pt-client` : la ligne doit etre rejetee au parse ou a la
    /// construction — jamais acceptee silencieusement.
    #[cfg(feature = "tor")]
    #[test]
    fn pont_pt_non_compile_rejete() {
        let lignes = vec![
            "obfs4 192.0.2.55:9001 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA cert=x iat-mode=0"
                .to_string(),
        ];
        for b in ponts_valides(&lignes) {
            // Si le parse a passe (transport reference accepte), le
            // build doit refuser faute de PT configure.
            assert!(b.build().is_err());
        }
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn embedded_sans_client_est_notready() {
        // Pas de bootstrap lance → pas de client arti → NotReady,
        // jamais de connexion clearnet de secours.
        let rt = TorRuntime::new(settings_with_policy());
        assert!(matches!(
            rt.fetch_onion(&format!("http://{TPB}/")).await,
            Err(TorError::NotReady)
        ));
    }
}
