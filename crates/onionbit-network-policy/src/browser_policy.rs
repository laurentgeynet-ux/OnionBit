// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Politique de cibles du **proxy navigateur** (ADR-0030 §4) —
//! inversee par rapport a l'allowlist du fetcher borne ADR-0028 :
//! un navigateur a une destination non bornee a priori, tout est
//! permis sauf l'interdit.
//!
//! Partagee entre `onionbit-tor::proxy` (decision a chaque requete
//! relayee) et `POST /api/browser/launch` (refus *avant* de lancer
//! le binaire) — une seule fonction, jamais deux implementations
//! qui divergent. Aucune resolution DNS n'existe dans ce chemin :
//! le hostname est valide par forme puis transmis brut a Tor.

use crate::PolicyError;

/// Mode de sortie clearnet du proxy navigateur (`tor.browser_proxy
/// .allow_clearnet_exit`, ADR-0030 §4) — tri-state tranche en 131.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClearnetExit {
    /// `.onion` uniquement — aucune destination clearnet (defaut,
    /// ligne ADR-0028 : jamais de sortie clearnet via un exit).
    #[default]
    Off,
    /// Clearnet vía exit **en HTTPS seulement** : `CONNECT :443`
    /// accepte (le contenu reste chiffre de bout en bout ; l'exit
    /// voit la destination — compromis documente §8.3). Tout HTTP
    /// clair (`GET` quel que soit le port, `CONNECT :80`) refuse.
    HttpsOnly,
    /// Clearnet vía exit ports {80,443} — l'exit voit le HTTP en
    /// clair : exige la decision explicite de l'utilisateur.
    On,
}

fn denied(reason: &'static str) -> PolicyError {
    PolicyError::DeniedDestination(reason)
}

/// Nom d'hote clearnet conforme a la grammaire DNS : labels
/// `[a-z0-9-]{1,63}` separes par des points, jamais de tiret en
/// tete/queue, total <= 253, **au moins un point** (un label nu —
/// `intranet`, `localhost` — est un nom d'infra interne, refuse
/// comme surface SSRF).
fn clearnet_host_ok(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|l| {
            (1..=63).contains(&l.len())
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

/// Politique navigateur appliquee a `(host, port)` deja decoupes.
///
/// Retourne `(host complet normalise, origine d'isolation)` :
///
/// - **`.onion`** : `OnionAddr::parse_host` — `*[label.]<56c>.onion`
///   v3 strict (checksum), sous-domaines toleres et conserves dans
///   le host (envoyes au pair par arti), origine = **adresse du
///   service** (cle first-party : `www.<base>` et `<base>` partagent
///   l'isolation). Un nom qui *finit* en `.onion` mais echoue au
///   parse v3 est refuse **dans tous les modes** — jamais de faux
///   `.onion` resolu par l'exit (`notreal.onion` via DNS public).
/// - **clearnet** : selon `exit` (`Off` refuse ; `HttpsOnly` exige
///   `is_connect && port == 443` — seul `CONNECT` garantit le TLS
///   de bout en bout cote client ; `On` ports {80,443}). Le nom
///   doit etre une forme DNS valide a au moins deux labels.
/// - **toujours refuses** : litteraux IP (v4/v6 — couvre loopback,
///   prive, link-local), `.onion` malforme, ports hors {80,443},
///   `is_connect=false` pour un port non-80.
///
/// `is_connect` distingue `CONNECT` (TLS etabli cote client) de la
/// requete `GET` en URI absolue (HTTP en clair jusqu'a l'exit).
pub fn check_browser_target(
    host: &str,
    port: u16,
    exit: ClearnetExit,
    is_connect: bool,
) -> Result<(String, String), PolicyError> {
    let host = host.trim().trim_end_matches('.');
    // Litteral IP refuse d'office — avant meme le parse `.onion`
    // (couvre v4/v6, loopback, prive, CGNAT, link-local).
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(denied("litteral IP interdit"));
    }
    let lower = host.to_ascii_lowercase();
    // Tout nom en `.onion` passe par le parse v3 strict — un
    // `.onion` malforme n'est JAMAIS traite en clearnet (un faux
    // `.onion` ne doit pas pouvoir sortir par l'exit en mode `On`).
    if lower == "onion" || lower.ends_with(".onion") {
        let (full, addr) = onionbit_format::OnionAddr::parse_host(&lower)
            .map_err(|_| denied("adresse .onion malformee"))?;
        if !matches!(port, 80 | 443) {
            return Err(denied("port hors {80, 443}"));
        }
        return Ok((full, addr.host().to_string()));
    }
    // Clearnet : refuse hors port {80,443} quel que soit le mode.
    if !matches!(port, 80 | 443) {
        return Err(denied("port hors {80, 443}"));
    }
    let allowed = match exit {
        ClearnetExit::Off => false,
        // `CONNECT :443` = TLS cote client, chiffre de bout en bout
        // au-dessus de l'exit ; `GET :443` en forme absolue reste du
        // HTTP clair → refuse.
        ClearnetExit::HttpsOnly => is_connect && port == 443,
        ClearnetExit::On => true,
    };
    if !allowed {
        return Err(denied("cible clearnet refusee (allow_clearnet_exit)"));
    }
    if !clearnet_host_ok(&lower) {
        return Err(denied("nom d'hote clearnet invalide"));
    }
    // Origine d'isolation = le host clearnet complet (un groupe de
    // circuits par site — pas de notion de service comme `.onion`).
    Ok((lower.clone(), lower))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TPB: &str = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";

    fn check(
        host: &str,
        port: u16,
        exit: ClearnetExit,
        connect: bool,
    ) -> Result<(String, String), PolicyError> {
        check_browser_target(host, port, exit, connect)
    }

    #[test]
    fn onion_accepte_dans_tous_les_modes() {
        for exit in [ClearnetExit::Off, ClearnetExit::HttpsOnly, ClearnetExit::On] {
            for connect in [false, true] {
                assert_eq!(
                    check(TPB, 443, exit, connect).unwrap(),
                    (TPB.into(), TPB.into())
                );
            }
        }
        // Sous-domaines : host complet conserve, origine = service.
        let sub = format!("www.{TPB}");
        assert_eq!(
            check(&sub, 443, ClearnetExit::Off, true).unwrap(),
            (sub, TPB.into())
        );
    }

    #[test]
    fn toujours_refuses_meme_en_mode_on() {
        // Refuses dans TOUS les modes, y compris `On` + CONNECT :443 :
        // litteraux IP, noms internes, `.onion` malforme (jamais de
        // sortie exit pour un nom finissant en `.onion`), labels
        // clearnet invalides.
        for bad in [
            "1.2.3.4",
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "::1",
            "localhost",
            "intranet",
            "pas-un.onion",
            "so_us.example.com",
            "-a.example.com",
            "a..example.com",
            "no-d ot.example.com",
            "pas56.onion",
        ] {
            for exit in [ClearnetExit::Off, ClearnetExit::HttpsOnly, ClearnetExit::On] {
                assert!(check(bad, 443, exit, true).is_err(), "{bad} en {exit:?}");
                assert!(check(bad, 80, exit, false).is_err(), "{bad} en {exit:?}");
            }
        }
        // Ports : {80,443} seulement, meme en `On`.
        assert!(check(TPB, 8080, ClearnetExit::On, true).is_err());
        assert!(check(TPB, 22, ClearnetExit::On, true).is_err());
        assert!(check("example.com", 8080, ClearnetExit::On, true).is_err());
    }

    #[test]
    fn clearnet_exit_tri_state() {
        // Off : tout clearnet refuse, .onion ok — `x.onion.evil.com`
        // est un nom clearnet (finit en `.com`), refuse comme tel.
        assert!(check("example.com", 443, ClearnetExit::Off, true).is_err());
        assert!(check("example.com", 80, ClearnetExit::Off, false).is_err());
        assert!(check("x.onion.evil.com", 443, ClearnetExit::Off, true).is_err());
        // HttpsOnly : CONNECT :443 ok, GET refuse MEME sur :443
        // (HTTP clair vers l'exit), CONNECT :80 refuse.
        assert_eq!(
            check("example.com", 443, ClearnetExit::HttpsOnly, true).unwrap(),
            ("example.com".into(), "example.com".into())
        );
        assert!(check("example.com", 443, ClearnetExit::HttpsOnly, false).is_err());
        assert!(check("example.com", 80, ClearnetExit::HttpsOnly, true).is_err());
        // On : CONNECT et GET {80,443} ok.
        assert!(check("example.com", 80, ClearnetExit::On, false).is_ok());
        assert!(check("example.com", 443, ClearnetExit::On, true).is_ok());
        // Normalisation casse + point terminal toleré.
        assert_eq!(
            check("WWW.Example.COM.", 443, ClearnetExit::On, true)
                .unwrap()
                .0,
            "www.example.com"
        );
    }
}
