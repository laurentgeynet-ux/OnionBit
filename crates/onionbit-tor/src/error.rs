// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs typees de `onionbit-tor`.

/// Erreur de la connectivite Tor.
#[derive(Debug, thiserror::Error)]
pub enum TorError {
    /// Le transport arti n'est pas compile (feature `tor` absente —
    /// ADR-0028 §1).
    #[error("transport Tor non compile (feature `tor` desactivee)")]
    NotBuilt,

    /// Tor est desactive (`tor.enabled = false`) — pas de repli
    /// possible : jamais de fallback clearnet.
    #[error("connectivite Tor desactivee")]
    Disabled,

    /// Le bootstrap Tor a echoue (reseau filtre, FAI bloquant, mode
    /// externe injoignable).
    #[error("bootstrap Tor echoue : {0}")]
    BootstrapFailed(String),

    /// Cible refusee par la politique d'allowlist onion.
    #[error("cible refusee : {0}")]
    DeniedTarget(#[from] onionbit_network_policy::PolicyError),

    /// Le client Tor n'est pas pret (bootstrap en cours ou jamais
    /// lance) — distinct de `Disabled` : la connectivite est voulue,
    /// le transport n'est pas encore disponible.
    #[error("client Tor non pret")]
    NotReady,

    /// Lancement ou gestion du service `.onion` impossible (feature
    /// `hs` absente, mode externe, keystore, lancement arti).
    #[error("service .onion : {0}")]
    OnionService(String),

    /// URL onion malformee ou scheme non supporte (v1 : `http://`
    /// seulement — pas de TLS sur un transport deja chiffre).
    #[error("URL onion invalide : {0}")]
    InvalidUrl(String),

    /// Reponse depassant `max_response_bytes` — la lecture est
    /// tronquee avant allocation excessive (service hostile ou
    /// configuration trop serree).
    #[error("reponse onion trop grande (>{limit} octets)")]
    TooLarge { limit: usize },

    /// Requete expiree (`request_timeout` — un `.onion` lent ne doit
    /// pas monopoliser de ressources).
    #[error("requete onion expiree")]
    Timeout,

    /// Redirection refusee : cible hors `http`, hors allowlist, ou
    /// trop de sauts (`max_redirects` depasse).
    #[error("redirection refusee : {0}")]
    RedirectDenied(String),

    /// Reponse HTTP indecodable par `httparse` (en-tetes corrompus,
    /// corps chunked tronque, …).
    #[error("reponse HTTP malformee : {0}")]
    MalformedResponse(String),

    /// Echec d'une requete vers un service cache.
    #[error("requete onion echouee : {0}")]
    Request(String),
}

/// Erreur typee d'une recherche externe `.onion` (ADR-0028 §5) —
/// chaque variante est surfaceable a l'UI : jamais un echec muet.
#[derive(Debug, thiserror::Error)]
pub enum ExternalSearchError {
    /// `tor.enabled = false` — la connectivite n'est pas voulue.
    #[error("connectivite Tor desactivee")]
    TorDisabled,

    /// Client Tor en cours de bootstrap ou transport non compile —
    /// retenter plus tard, ce n'est pas une faute de requete.
    #[error("client Tor non pret")]
    TorNotReady,

    /// Tous les miroirs ont echoue cote transport (down, timeout,
    /// reponse corrompue, statut inattendu sans defi).
    #[error("tous les miroirs injoignables ({tried} tentes)")]
    MirrorUnreachable { tried: usize },

    /// Page de verification anti-DDoS / captcha / PoW detectee —
    /// frequente sur les onion a fort trafic.
    #[error("protection anti-DDoS detectee (captcha/PoW)")]
    CaptchaDetected,

    /// Page servie mais les selecteurs n'extraient rien — refonte
    /// probable du site.
    #[error("structure de page inattendue — refonte probable")]
    StructureChanged,

    /// Reponse valide, le site annonce explicitement zero resultat.
    #[error("aucun resultat")]
    NoResults,

    /// Requete vide apres nettoyage.
    #[error("requete de recherche vide")]
    EmptyQuery,

    /// `source` absente du registre de backends.
    #[error("backend de recherche inconnu : {0}")]
    UnknownSource(String),
}

/// Alias de `Result` pour ce crate.
pub type Result<T> = std::result::Result<T, TorError>;
