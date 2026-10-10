//! Compteurs d'E/S positionnees (pread/pwrite) — instrumentation
//! ADR-0023 etape 84 : mesurer la chute du read-back de verification
//! apres l'introduction du hash en RAM (`check_piece_data`).
//!
//! Globaux par processus (le stockage n'a pas de reference vers la
//! session) : `Session::stats_snapshot` les expose dans
//! `SessionStatsSnapshot.storage_io` ; `reset` pour les mesures
//! avant/apres ciblees (tests, bancs).

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

static PREAD_OPS: AtomicU64 = AtomicU64::new(0);
static PREAD_BYTES: AtomicU64 = AtomicU64::new(0);
static PWRITE_OPS: AtomicU64 = AtomicU64::new(0);
static PWRITE_BYTES: AtomicU64 = AtomicU64::new(0);

/// Instantane des compteurs d'E/S positionnees.
#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct IoCounters {
    /// Nombre d'appels `pread` reussis.
    pub pread_ops: u64,
    /// Octets lus par `pread`.
    pub pread_bytes: u64,
    /// Nombre d'appels `pwrite`/`pwritev` reussis.
    pub pwrite_ops: u64,
    /// Octets ecrits par `pwrite`/`pwritev`.
    pub pwrite_bytes: u64,
}

/// Instantane global des compteurs d'E/S positionnees.
pub fn io_counters() -> IoCounters {
    IoCounters {
        pread_ops: PREAD_OPS.load(Ordering::Relaxed),
        pread_bytes: PREAD_BYTES.load(Ordering::Relaxed),
        pwrite_ops: PWRITE_OPS.load(Ordering::Relaxed),
        pwrite_bytes: PWRITE_BYTES.load(Ordering::Relaxed),
    }
}

/// Remet les compteurs a zero — mesures bornees (bancs, tests).
pub fn reset_io_counters() {
    PREAD_OPS.store(0, Ordering::Relaxed);
    PREAD_BYTES.store(0, Ordering::Relaxed);
    PWRITE_OPS.store(0, Ordering::Relaxed);
    PWRITE_BYTES.store(0, Ordering::Relaxed);
}

/// Compteurs partages d'une instance de stockage — immunises aux
/// autres sessions/tests paralleles (contrairement aux globaux,
/// qui restent l'exposition `SessionStatsSnapshot`).
#[derive(Debug, Default)]
pub struct IoCountersShared {
    pread_ops: AtomicU64,
    pread_bytes: AtomicU64,
    pwrite_ops: AtomicU64,
    pwrite_bytes: AtomicU64,
}

impl IoCountersShared {
    /// Instantane de cette instance de stockage uniquement.
    pub fn snapshot(&self) -> IoCounters {
        IoCounters {
            pread_ops: self.pread_ops.load(Ordering::Relaxed),
            pread_bytes: self.pread_bytes.load(Ordering::Relaxed),
            pwrite_ops: self.pwrite_ops.load(Ordering::Relaxed),
            pwrite_bytes: self.pwrite_bytes.load(Ordering::Relaxed),
        }
    }

    /// Remet les compteurs de l'instance a zero.
    pub fn reset(&self) {
        self.pread_ops.store(0, Ordering::Relaxed);
        self.pread_bytes.store(0, Ordering::Relaxed);
        self.pwrite_ops.store(0, Ordering::Relaxed);
        self.pwrite_bytes.store(0, Ordering::Relaxed);
    }

    /// Comptabilise un `pread` reussi (instance + globaux).
    pub(crate) fn count_pread(&self, bytes: u64) {
        self.pread_ops.fetch_add(1, Ordering::Relaxed);
        self.pread_bytes.fetch_add(bytes, Ordering::Relaxed);
        PREAD_OPS.fetch_add(1, Ordering::Relaxed);
        PREAD_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Comptabilise un `pwrite`/`pwritev` reussi (instance + globaux).
    pub(crate) fn count_pwrite(&self, bytes: u64) {
        self.pwrite_ops.fetch_add(1, Ordering::Relaxed);
        self.pwrite_bytes.fetch_add(bytes, Ordering::Relaxed);
        PWRITE_OPS.fetch_add(1, Ordering::Relaxed);
        PWRITE_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }
}
