//! Kill switch atomique.
//!
//! Coupe toute action dependant de l'anonymat quand le niveau
//! attendu n'est plus garanti (ex. circuit anonyme mort, policy
//! degradee, proxy SOCKS5 injoignable). Partage en `Arc<KillSwitch>`
//! entre les crates.
//!
//! L'engagement est **par source** (`engage_scoped`/`release_scoped`) :
//! le switch reste engage tant qu'au moins une source signale une
//! panne — un proxy redevenu joignable ne desarme pas une lane dont
//! les circuits sont encore morts, et reciproquement.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{PolicyError, Result};

/// Portee par defaut des engagements non scoppes (`engage`/`release`).
const SCOPE_MANUAL: &str = "manuel";

/// Kill switch partage : quand il est engage, tout envoi
/// conditionne par l'anonymat doit etre refuse via [`Self::guard`].
#[derive(Debug, Default)]
pub struct KillSwitch {
    /// Raisons d'engagement actives par portee (`scope -> reason`).
    scopes: Mutex<HashMap<String, String>>,
}

impl KillSwitch {
    /// Cree un kill switch desarme.
    pub fn new() -> Self {
        Self::default()
    }

    /// Engage le kill switch pour la portee `scope` (`reason` sert au
    /// diagnostic/log). Idempotent par portee.
    pub fn engage_scoped(&self, scope: &str, reason: impl Into<String>) {
        let mut scopes = self.scopes.lock().unwrap();
        if scopes.insert(scope.to_string(), reason.into()).is_none() && scopes.len() == 1 {
            tracing::warn!(scope, "kill switch engage");
        }
    }

    /// Desarme la portee `scope` ; le switch reste engage si une
    /// autre portee est encore active. Retourne `true` si la portee
    /// etait engagee.
    pub fn release_scoped(&self, scope: &str) -> bool {
        let mut scopes = self.scopes.lock().unwrap();
        let was = scopes.remove(scope).is_some();
        if was && scopes.is_empty() {
            tracing::info!("kill switch desarme");
        }
        was
    }

    /// Engage le kill switch (portee `"manuel"` — compatibilite).
    pub fn engage(&self, reason: impl Into<String>) {
        self.engage_scoped(SCOPE_MANUAL, reason);
    }

    /// Desarme **toutes** les portees (retour manuel = decision
    /// operateur, elle l'emporte sur les sources restantes).
    pub fn release(&self) {
        let mut scopes = self.scopes.lock().unwrap();
        if !scopes.is_empty() {
            scopes.clear();
            tracing::info!("kill switch desarme (manuel)");
        }
    }

    /// `true` si engage (au moins une portee active).
    pub fn is_engaged(&self) -> bool {
        !self.scopes.lock().unwrap().is_empty()
    }

    /// Raisons des engagements actifs (diagnostic).
    pub fn reason(&self) -> Option<String> {
        let scopes = self.scopes.lock().unwrap();
        if scopes.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = scopes.iter().map(|(s, r)| format!("{s}: {r}")).collect();
        parts.sort();
        Some(parts.join(" ; "))
    }

    /// Garde-fou : `Err(KillSwitchEngaged)` si engage — a appeler
    /// avant tout envoi de trafic dependant de l'anonymat.
    pub fn guard(&self) -> Result<()> {
        if self.is_engaged() {
            return Err(PolicyError::KillSwitchEngaged(
                self.reason().unwrap_or_else(|| "raison inconnue".into()),
            ));
        }
        Ok(())
    }

    /// Garde-fou lors de l'ajout d'un téléchargement : bloque si le proxy
    /// SOCKS5 local lui-même est inaccessible ou si un arrêt d'urgence explicite
    /// a été déclenché, mais n'empêche pas l'enregistrement d'un téléchargement
    /// en attente de circuits ("le téléchargement attendra").
    pub fn guard_add(&self) -> Result<()> {
        let scopes = self.scopes.lock().unwrap();
        for (scope, reason) in scopes.iter() {
            if scope != "circuits" {
                return Err(PolicyError::KillSwitchEngaged(format!("{scope}: {reason}")));
            }
        }
        Ok(())
    }

    /// `true` si la portée `scope` est engagée.
    pub fn is_scope_engaged(&self, scope: &str) -> bool {
        self.scopes.lock().unwrap().contains_key(scope)
    }

    /// Garde-fou ciblé sur une portée : `Err(KillSwitchEngaged)` si `scope` est engagée.
    pub fn guard_scope(&self, scope: &str) -> Result<()> {
        let scopes = self.scopes.lock().unwrap();
        if let Some(reason) = scopes.get(scope) {
            return Err(PolicyError::KillSwitchEngaged(format!("{scope}: {reason}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engage_bloque_guard_release_debloque() {
        let ks = KillSwitch::new();
        assert!(ks.guard().is_ok());
        ks.engage("test");
        assert!(ks.is_engaged());
        assert!(matches!(ks.guard(), Err(PolicyError::KillSwitchEngaged(_))));
        assert_eq!(ks.reason().as_deref(), Some("manuel: test"));
        ks.release();
        assert!(ks.guard().is_ok());
    }

    #[test]
    fn scopes_independants_une_portee_reste_engagee() {
        let ks = KillSwitch::new();
        ks.engage_scoped("proxy", "proxy mort");
        ks.engage_scoped("circuits", "aucun circuit");
        assert!(ks.is_engaged());
        // Lever le proxy ne suffit pas : les circuits restent morts.
        ks.release_scoped("proxy");
        assert!(ks.is_engaged());
        assert!(ks.reason().unwrap().contains("circuits"));
        ks.release_scoped("circuits");
        assert!(!ks.is_engaged());
        assert!(ks.guard().is_ok());
    }
}
