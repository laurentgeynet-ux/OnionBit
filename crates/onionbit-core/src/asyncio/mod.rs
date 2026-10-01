//! `AsyncioEndpoint` (`pyipv8/ipv8/REST/asyncio_endpoint.py`) adapte
//! au runtime tokio :
//!
//! - [`AsyncioMonitor::enable_drift`]/[`AsyncioMonitor::disable_drift`]
//!   — `DriftMeasurementStrategy` : tache `interval(walker_interval)`
//!   mesurant l'ecart entre deux ticks (historique borne 100).
//! - [`TaskRegistry`] — substitut de `all_tasks()` pour `/tasks`.
//! - [`debug_log`] — `DequeLogHandler` + `EnvFilter` rechargé pour
//!   `/debug`.

pub mod debug_log;
pub mod monitor;
pub mod tasks;

pub use debug_log::{debug_log, set_filter_reload, DebugLogBuffer, DebugLogLayer};
pub use monitor::AsyncioMonitor;
pub use tasks::{NamedTask, TaskRegistry};
