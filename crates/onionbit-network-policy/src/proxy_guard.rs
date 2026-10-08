// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validation de la cible du proxy SOCKS5 local.
//!
//! `EngineConfig.socks5_proxy` ne doit JAMAIS pointer vers un proxy
//! distant non verifie : sinon les connexions pairs sortent en clair
//! hors des tunnels. Ce module borne les URL `socks5://`/`socks5h://`
//! a des adresses loopback.

use std::net::SocketAddr;

use crate::error::{PolicyError, Result};

/// Valide une URL `socks5://`/`socks5h://` : doit designer une
/// adresse `host:port` **loopback** numerique (le proxy local expose
/// par `onionbit-tunnel`). Rejette les DNS, IPs non-loopback et les
/// URLs avec identifiants/chemins.
pub fn validate_local_socks5_url(url: &str) -> Result<SocketAddr> {
    let rest = url
        .strip_prefix("socks5://")
        .or_else(|| url.strip_prefix("socks5h://"))
        .ok_or(PolicyError::InvalidProxy("schema socks5:// attendu"))?;
    if rest.contains('@') || rest.contains('/') {
        return Err(PolicyError::InvalidProxy(
            "identifiants ou chemin interdits dans l'URL du proxy",
        ));
    }
    let addr: SocketAddr = rest
        .parse()
        .map_err(|_| PolicyError::InvalidProxy("host:port numerique attendu"))?;
    if !addr.ip().is_loopback() {
        return Err(PolicyError::DeniedDestination(
            "le proxy SOCKS5 doit etre local (loopback)",
        ));
    }
    if addr.port() == 0 {
        return Err(PolicyError::InvalidProxy("port de proxy nul"));
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepte_loopback_numerique() {
        assert_eq!(
            validate_local_socks5_url("socks5://127.0.0.1:9050").unwrap(),
            "127.0.0.1:9050".parse::<SocketAddr>().unwrap()
        );
        assert!(validate_local_socks5_url("socks5h://[::1]:1080").is_ok());
    }

    #[test]
    fn refuse_proxy_distant_et_formes_dangereuses() {
        for url in [
            "socks5://8.8.8.8:1080",
            "http://127.0.0.1:9050",
            "socks5://localhost:9050",
            "socks5://user:pw@127.0.0.1:9050",
            "socks5://127.0.0.1:9050/path",
            "socks5://127.0.0.1:0",
        ] {
            assert!(
                validate_local_socks5_url(url).is_err(),
                "{url} devrait etre refuse"
            );
        }
    }
}
