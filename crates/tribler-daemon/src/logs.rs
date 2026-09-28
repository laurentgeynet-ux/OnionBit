//! Rotation « un fichier par run » des logs du daemon.
//!
//! `tracing_appender::rolling::daily` concatene tous les runs du jour
//! dans `tribler.log.YYYY-MM-DD` — illisible quand on cherche les logs
//! du run courant (onglet Diagnostic / `/api/logging`). Ici
//! `rolling::never` ecrit toujours `tribler.log` et [`rotate`]
//! archive le run precedent au demarrage.

use std::path::{Path, PathBuf};

/// Archive le `tribler.log` du run precedent et purge les archives en
/// exces. A appeler AVANT la creation de l'appender (le fichier
/// courant doit etre ferme).
///
/// - `tribler.log` → `tribler.log.1`, les archives numerotees
///   existantes sont decalees (`.N` → `.N+1`, `.max_files` disparait) ;
/// - `max_files = 0` : le fichier precedent est simplement supprime ;
/// - les anciens fichiers datees `tribler.log.YYYY-MM-DD` (rotation
///   quotidienne precedente) ont leur propre retention `max_files`,
///   triee par date de modification.
pub fn rotate(logs_dir: &Path, max_files: usize) {
    let current = logs_dir.join("tribler.log");
    if current.exists() {
        if max_files == 0 {
            let _ = std::fs::remove_file(&current);
        } else {
            // L'archive la plus ancienne sort de la retention, puis
            // decalage du plus grand indice vers le plus petit.
            let _ = std::fs::remove_file(logs_dir.join(format!("tribler.log.{max_files}")));
            for n in (1..max_files).rev() {
                let src = logs_dir.join(format!("tribler.log.{n}"));
                if src.exists() {
                    let _ = std::fs::rename(&src, logs_dir.join(format!("tribler.log.{}", n + 1)));
                }
            }
            let _ = std::fs::rename(&current, logs_dir.join("tribler.log.1"));
        }
    }
    // Purge des archives hors retention : numerotees `.N` avec
    // `N > max_files` (residu d'une config passee a une retention
    // plus basse) et datees au-dela de `max_files` (par recence).
    let mut dated: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(logs_dir)
        .map(|rd| rd.flatten().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(suffix) = name.strip_prefix("tribler.log.") else {
            continue;
        };
        if suffix.bytes().all(|b| b.is_ascii_digit()) {
            if suffix.parse::<usize>().is_ok_and(|n| n > max_files) {
                let _ = std::fs::remove_file(entry.path());
            }
        } else {
            dated.push(entry.path());
        }
    }
    dated.sort_by_cached_key(|p| {
        std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok())
    });
    for path in dated.into_iter().skip(max_files) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::rotate;

    fn fichiers(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn archive_le_run_precedent_et_decale_les_numerotees() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path();
        std::fs::write(logs.join("tribler.log"), "run3").unwrap();
        std::fs::write(logs.join("tribler.log.1"), "run2").unwrap();
        std::fs::write(logs.join("tribler.log.2"), "run1").unwrap();
        rotate(logs, 3);
        assert_eq!(
            std::fs::read_to_string(logs.join("tribler.log.1")).unwrap(),
            "run3"
        );
        assert_eq!(
            std::fs::read_to_string(logs.join("tribler.log.2")).unwrap(),
            "run2"
        );
        assert_eq!(
            std::fs::read_to_string(logs.join("tribler.log.3")).unwrap(),
            "run1"
        );
        assert!(!logs.join("tribler.log").exists());
    }

    #[test]
    fn l_archive_la_plus_ancienne_sort_de_la_retention() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path();
        std::fs::write(logs.join("tribler.log"), "run3").unwrap();
        std::fs::write(logs.join("tribler.log.1"), "run2").unwrap();
        std::fs::write(logs.join("tribler.log.2"), "run1").unwrap();
        rotate(logs, 2);
        assert_eq!(fichiers(logs), ["tribler.log.1", "tribler.log.2"]);
        assert_eq!(
            std::fs::read_to_string(logs.join("tribler.log.2")).unwrap(),
            "run2"
        );
    }

    #[test]
    fn purge_les_datees_au_dela_de_max_files() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path();
        std::fs::write(logs.join("tribler.log"), "run").unwrap();
        std::fs::write(logs.join("tribler.log.2026-01-01"), "a").unwrap();
        std::fs::write(logs.join("tribler.log.2026-01-02"), "b").unwrap();
        rotate(logs, 1);
        let restants = fichiers(logs);
        assert_eq!(restants.len(), 2);
        assert!(restants.contains(&"tribler.log.1".to_string()));
        assert!(restants.iter().any(|n| n.starts_with("tribler.log.2026")));
    }

    #[test]
    fn max_files_zero_supprime_tout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tribler.log"), "run").unwrap();
        std::fs::write(dir.path().join("tribler.log.1"), "run-1").unwrap();
        std::fs::write(dir.path().join("tribler.log.2026-01-01"), "d").unwrap();
        rotate(dir.path(), 0);
        assert!(fichiers(dir.path()).is_empty());
    }

    #[test]
    fn sans_fichier_courant_ne_fait_rien() {
        let dir = tempfile::tempdir().unwrap();
        rotate(dir.path(), 5);
        assert!(fichiers(dir.path()).is_empty());
    }
}
