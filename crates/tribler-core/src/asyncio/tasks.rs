//! Registre des taches nommees du daemon — equivalent de
//! `asyncio.all_tasks()` pour `GET /api/ipv8/asyncio/tasks`.
//!
//! Tokio n'offre pas d'introspection des taches : chaque composant
//! enregistre ses taches de fond nommees (equivalent du
//! `TaskManager.register_task("Class:task", interval=...)` de
//! pyipv8). Les taches periodiques declarent leur `interval` (s) ;
//! les taches anonymes (`register_anonymous_task`) n'ont ni
//! `taskmanager` ni `interval`.

use std::sync::{Arc, Mutex};

/// Secondes Unix avec precision flottante (`time.time()` Python).
pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Une tache enregistree — serialisee en
/// `{name, running, stack, taskmanager?, start_time?, interval?}`.
#[derive(Debug, Clone)]
pub struct NamedTask {
    /// Nom de la tache (partie apres `":"` du nom Python).
    pub name: String,
    /// Classe du `TaskManager` Python (`"DHTDiscoveryCommunity"`,
    /// `"CoreSession"`, ...) — absent pour les taches anonymes.
    pub taskmanager: Option<String>,
    /// `task.start_time` — epoch flottant.
    pub start_time: f64,
    /// `task.interval` (s) — taches periodiques `register_task`.
    pub interval: Option<f64>,
}

/// Registre partageable (`Clone`) — un seul par session.
#[derive(Clone, Default)]
pub struct TaskRegistry {
    tasks: Arc<Mutex<Vec<NamedTask>>>,
}

impl TaskRegistry {
    /// Enregistre une tache nommee ; une re-inscription au meme nom
    /// remplace l'entree (nouveau `start_time`).
    pub fn register(&self, taskmanager: Option<&str>, name: &str, interval: Option<f64>) {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.retain(|t| !(t.taskmanager.as_deref() == taskmanager && t.name == name));
        tasks.push(NamedTask {
            name: name.to_string(),
            taskmanager: taskmanager.map(str::to_string),
            start_time: now_secs(),
            interval,
        });
    }

    /// Instantane pour l'endpoint REST.
    pub fn snapshot(&self) -> Vec<NamedTask> {
        self.tasks.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_dedup_snapshot() {
        let reg = TaskRegistry::default();
        reg.register(Some("CoreSession"), "progress", Some(0.5));
        reg.register(None, "bootstrap", None);
        reg.register(Some("CoreSession"), "progress", Some(0.5));
        let tasks = reg.snapshot();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].taskmanager.as_deref(), Some("CoreSession"));
        assert_eq!(tasks[1].interval, Some(0.5));
        assert!(tasks[1].start_time > 0.0);
    }
}
