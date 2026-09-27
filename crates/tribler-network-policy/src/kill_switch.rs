//! Kill switch atomique.
//!
//! Coupe toute action dependant de l'anonymat quand le niveau
//! attendu n'est plus garanti (ex. circuit anonyme mort, policy
//! degradee). Partage en `Arc<KillSwitch>` entre les crates ;
//! l'engagement est **atomique** (`AtomicBool`) — aucun race entre
//! le declenchement et un envoi de paquet n'est possible.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::error::{PolicyError, Result};

/// Kill switch partage : quand il est engage, tout envoi
/// conditionne par l'anonymat doit etre refuse via [`Self::guard`].
#[derive(Debug, Default)]
pub struct KillSwitch {
    engaged: AtomicBool,
    /// Raison du dernier engagement (diagnostic — ex.
    /// "circuit anonyme perdu").
    reason: Mutex<Option<String>>,
}

impl KillSwitch {
    /// Cree un kill switch desarme.
    pub fn new() -> Self {
        Self::default()
    }

    /// Engage le kill switch (`reason` sert au diagnostic/log).
    /// Idempotent : seule la premiere raison est conservee.
    pub fn engage(&self, reason: impl Into<String>) {
        if !self.engaged.swap(true, Ordering::SeqCst) {
            *self.reason.lock().unwrap() = Some(reason.into());
            tracing::warn!("kill switch engage");
        }
    }

    /// Desarme le kill switch (retour manuel, ex. apres retablissement
    /// d'un circuit anonyme).
    pub fn release(&self) {
        self.engaged.store(false, Ordering::SeqCst);
        *self.reason.lock().unwrap() = None;
    }

    /// `true` si engage.
    pub fn is_engaged(&self) -> bool {
        self.engaged.load(Ordering::SeqCst)
    }

    /// Raison du dernier engagement.
    pub fn reason(&self) -> Option<String> {
        self.reason.lock().unwrap().clone()
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
        assert_eq!(ks.reason().as_deref(), Some("test"));
        ks.release();
        assert!(ks.guard().is_ok());
    }
}
