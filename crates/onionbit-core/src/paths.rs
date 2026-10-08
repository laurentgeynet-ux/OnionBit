// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Racines portables et grammaire des chemins persistes (ADR-0018,
//! etape 57).
//!
//! Pour qu'un bundle survive a un changement de lettre de lecteur ou
//! de point de montage, aucun chemin absolu machine ne doit rester
//! fige dans `configuration.json`, `onionbit.db` ou `session.json`.
//! Les valeurs persistees sous une racine connue s'ecrivent en spec
//! portable — `@state/…`, `@public/…`, `@private/…` — resolue contre
//! les racines effectives du run courant.
//!
//! Grammaire stricte d'un spec : `@racine/rel/a/b`
//!
//! - racines : `state` (etat interne), `public` (donnees en clair),
//!   `private` (donnees chiffrees liees a l'identite) ;
//! - separateur `/` uniquement — `\` (mixte Windows) est rejete ;
//! - composants interdits : `.`, `..`, `:` (lettre de lecteur / flux
//!   ADS) et `NUL` ; les composants vides (`//`, `/` final) sont
//!   absorbes par normalisation — sans danger puisque rien ne peut
//!   remonter ou sortir de la racine ;
//! - `@` seul ou tout autre jeton `@xxx` est refuse — jamais pris
//!   pour un chemin relatif ordinaire.
//!
//! Un chemin **non** prefixe `@` n'est pas un spec : il passe
//! inchange (absolu externe explicite ou relatif historique resolu
//! contre `state_dir` par les consommateurs, comme avant).

use std::path::{Component, Path, PathBuf};

/// Nom du fichier marqueur a la racine d'un bundle portable
/// (`<root>/OnionBit.portable`, ADR-0018 etape 58 — sa presence fait
/// remonter `state/` et `data/` a la racine du bundle).
pub const PORTABLE_MARKER: &str = "OnionBit.portable";

const TOKENS: [(&str, PortableRoot); 3] = [
    ("@state", PortableRoot::State),
    ("@public", PortableRoot::Public),
    ("@private", PortableRoot::Private),
];

/// Racine canonique d'un spec portable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableRoot {
    /// `state/` — base, config, logs, fastresume, identite.
    State,
    /// `data/public` — telechargements et medias en clair.
    Public,
    /// `data/private` — zone chiffree liee a l'identite.
    Private,
}

impl PortableRoot {
    /// Jeton du spec (`"@state"`…).
    pub fn token(self) -> &'static str {
        match self {
            Self::State => "@state",
            Self::Public => "@public",
            Self::Private => "@private",
        }
    }

    fn from_token(tok: &str) -> Option<Self> {
        TOKENS.iter().find(|(t, _)| *t == tok).map(|(_, r)| *r)
    }
}

/// Erreur de validation d'un spec portable.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathSpecError {
    /// Jeton `@xxx` inconnu — les seules racines sont `state`,
    /// `public`, `private`.
    #[error("racine portable inconnue « {0} » (attendu @state, @public ou @private)")]
    UnknownRoot(String),
    /// Composant invalide (`..`, `.`, `\\`, `:`, vide, NUL…).
    #[error("spec portable « {0} » invalide : {1}")]
    Invalid(String, &'static str),
}

/// Racines de resolution du run courant — derivees de `state_dir`
/// (aucun chemin machine n'est re-persiste : seules les specs
/// `@root/…` voyagent).
#[derive(Debug, Clone)]
pub struct PathRoots {
    state: PathBuf,
    public: PathBuf,
    private: PathBuf,
}

/// Resultat de la migration d'une valeur persistee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathMigration {
    /// Valeur conservee (vide, relative, spec valide).
    Keep,
    /// Reecrire la valeur avec ce spec portable.
    Rewrite(String),
    /// Chemin absolu hors des racines : conserve tel quel — choix
    /// externe explicite de l'utilisateur (a journaliser).
    External,
    /// Spec `@…` mal forme : conserve, a journaliser en `warn`.
    Invalid,
}

impl PathRoots {
    /// Racines derivees de `state_dir`. Le dossier `data/` est
    /// **voisin** de `state/` quand celui-ci s'appelle `state`
    /// (bundle portable `<root>/state` → `<root>/data`) ; sinon il
    /// vit sous `state_dir` (`<state_dir>/data` — `.onionbit` dev,
    /// `--state-dir` explicite), ce qui preserve l'auto-contenance.
    pub fn for_state_dir(state_dir: &Path) -> Self {
        let data = if state_dir
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("state"))
        {
            state_dir
                .parent()
                .map(|p| p.join("data"))
                .unwrap_or_else(|| state_dir.join("data"))
        } else {
            state_dir.join("data")
        };
        Self {
            state: state_dir.to_path_buf(),
            public: data.join("public"),
            private: data.join("private"),
        }
    }

    /// Racines explicites (tests, layouts alternatifs).
    pub fn from_parts(state: PathBuf, public: PathBuf, private: PathBuf) -> Self {
        Self {
            state,
            public,
            private,
        }
    }

    /// `state/`.
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// `data/` — parent commun des zones public/prive.
    pub fn data(&self) -> PathBuf {
        self.public
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    }

    /// `data/public`.
    pub fn public(&self) -> &Path {
        &self.public
    }

    /// `data/private`.
    pub fn private(&self) -> &Path {
        &self.private
    }

    /// `data/public/temp` — fichiers en cours (avant
    /// `move_on_completion`).
    pub fn public_temp(&self) -> PathBuf {
        self.public.join("temp")
    }

    /// `data/public/downloads` — fichiers termines.
    pub fn public_downloads(&self) -> PathBuf {
        self.public.join("downloads")
    }

    /// `data/public/torrents` — sauvegardes `.torrent` (`torrent_folder`).
    pub fn public_torrents(&self) -> PathBuf {
        self.public.join("torrents")
    }

    /// `data/private/temp` — zone chiffree, fichiers en cours.
    pub fn private_temp(&self) -> PathBuf {
        self.private.join("temp")
    }

    /// `data/private/downloads` — zone chiffree, fichiers termines.
    pub fn private_downloads(&self) -> PathBuf {
        self.private.join("downloads")
    }

    /// Repertoire de la racine.
    fn dir_of(&self, root: PortableRoot) -> &Path {
        match root {
            PortableRoot::State => &self.state,
            PortableRoot::Public => &self.public,
            PortableRoot::Private => &self.private,
        }
    }

    /// Resout `spec` en chemin de ce run.
    ///
    /// - `@root/…` : resolution stricte sous la racine — erreur sur
    ///   composant hostile (jamais de `..`, jamais hors racine par
    ///   construction) ;
    /// - autre valeur : pas un spec — retournee comme `PathBuf`
    ///   litteral (compat : absolu externe, relatif legacy).
    pub fn resolve(&self, spec: &str) -> Result<PathBuf, PathSpecError> {
        if spec.starts_with('@') {
            let (root, rel) = parse_spec(spec)?;
            let mut out = self.dir_of(root).to_path_buf();
            for comp in rel {
                out.push(comp);
            }
            return Ok(out);
        }
        Ok(PathBuf::from(spec))
    }

    /// Valeur persistee → chemin du run courant. Un spec `@…`
    /// invalide est logge puis conserve litteralement — ne jamais
    /// perdre la valeur de l'utilisateur sur une entree degradee.
    pub fn resolve_persisted(&self, stored: &str) -> PathBuf {
        match self.resolve(stored) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    spec = stored,
                    error = %e,
                    "chemin persiste invalide — valeur litterale conservee"
                );
                PathBuf::from(stored)
            }
        }
    }

    /// Chemin saisi par l'utilisateur/API : un `@…` est un spec
    /// strict (erreur si mal forme), un chemin ordinaire est repris
    /// tel quel.
    pub fn resolve_input(&self, p: &Path) -> Result<PathBuf, PathSpecError> {
        if let Some(s) = p.to_str() {
            if s.starts_with('@') {
                return self.resolve(s);
            }
        }
        Ok(p.to_path_buf())
    }

    /// `path` sous une racine connue → son spec `@root/…`
    /// (`None` si hors racines — chemin externe a conserver absolu).
    ///
    /// Comparaison lexicale normalisee (`.` elimines, `..` reduits
    /// sans acces disque) ; insensible a la casse sous Windows (la
    /// lettre de lecteur `E:`/`e:` et les noms ne doivent pas faire
    /// echouer la detection apres un remontage).
    pub fn to_portable(&self, path: &Path) -> Option<String> {
        // `private`/`public` avant `state` : quand `data/` vit sous
        // `state_dir` (layout non portable), un chemin de zone doit
        // matcher sa racine specifique et non `@state/data/…`.
        for (root, dir) in [
            (PortableRoot::Private, &self.private),
            (PortableRoot::Public, &self.public),
            (PortableRoot::State, &self.state),
        ] {
            if let Some(rel) = strip(path, dir) {
                return Some(if rel.is_empty() {
                    root.token().to_string()
                } else {
                    format!("{}/{}", root.token(), rel.join("/"))
                });
            }
        }
        None
    }

    /// Migration d'une valeur persistee vers la grammaire portable
    /// (etape 57) — idempotente, reappliquee a chaque chargement.
    ///
    /// - spec `@` valide ou valeur vide/relative → [`PathMigration::Keep`] ;
    /// - absolu sous une racine (ou sous un emplacement legacy de
    ///   `state/`, deplace par le layout etape 58) → [`PathMigration::Rewrite`] ;
    /// - absolu hors racines → [`PathMigration::External`] (choix
    ///   explicite conserve) ;
    /// - spec `@` mal forme → [`PathMigration::Invalid`].
    pub fn migrate_persisted(&self, stored: &str) -> PathMigration {
        if stored.is_empty() {
            return PathMigration::Keep;
        }
        if stored.starts_with('@') {
            return match parse_spec(stored) {
                Ok(_) => PathMigration::Keep,
                Err(_) => PathMigration::Invalid,
            };
        }
        let p = Path::new(stored);
        if !p.has_root() {
            // Relatif historique : resolu contre `state_dir` a
            // l'usage — deja neutre en chemin machine. `has_root`
            // plutot qu'`is_absolute` : sous Windows `/x` (racine
            // relative sans lecteur) et `\\?\…` comptent comme
            // ancres, `C:x` (lecteur-relatif) reste un relatif.
            return PathMigration::Keep;
        }
        if let Some(spec) = self.legacy_or_rooted_spec(p) {
            return PathMigration::Rewrite(spec);
        }
        PathMigration::External
    }

    /// Spec d'un chemin absolu sous une racine, avec remap des
    /// emplacements **legacy** deplaces par le layout etape 58 :
    ///
    /// - `<state>/downloads/…` → `@public/downloads/…` (les donnees
    ///   quittent `state/`) ;
    /// - `<state>/torrents/…` → `@public/torrents/…` ;
    /// - fichiers d'identite a la racine de `<state>` →
    ///   `@state/identity/<fichier>` ;
    /// - autre chemin sous `<state>` → `@state/…`.
    fn legacy_or_rooted_spec(&self, p: &Path) -> Option<String> {
        for (root, dir) in [
            (PortableRoot::Private, &self.private),
            (PortableRoot::Public, &self.public),
        ] {
            if let Some(rel) = strip(p, dir) {
                return Some(rooted(root, &rel));
            }
        }
        strip(p, &self.state).map(|rel| match rel.first().map(String::as_str) {
            Some("downloads") => rooted(PortableRoot::Public, &rel),
            Some("torrents") => rooted(PortableRoot::Public, &rel),
            Some(name) if rel.len() == 1 && IDENTITY_FILES.contains(&name) => rooted(
                PortableRoot::State,
                &["identity".to_string(), name.to_string()],
            ),
            _ => rooted(PortableRoot::State, &rel),
        })
    }
}

/// Fichiers d'identite deplaces sous `state/identity/` par le layout
/// etape 58 (un chemin persiste qui y pointe encore doit suivre).
const IDENTITY_FILES: &[&str] = &[
    "identity_seed.bin",
    "ipv8_keypair.bin",
    "stealth_bridge.key",
    "secondary_key.pem",
    "ec_multichain.pem",
    "ecpub_multichain.pem",
];

fn rooted(root: PortableRoot, rel: &[String]) -> String {
    if rel.is_empty() {
        root.token().to_string()
    } else {
        format!("{}/{}", root.token(), rel.join("/"))
    }
}

/// Cherche `OnionBit.portable` en remontant depuis le dossier de
/// `exe` (layout `<root>/<os>/onionbit-daemon.exe`) jusqu'aux
/// ancetres — la racine du bundle portable si trouvee.
pub fn find_portable_root(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent();
    while let Some(d) = dir {
        if d.join(PORTABLE_MARKER).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// `true` si `s` a la forme d'un spec portable (prefixe `@`) —
/// meme invalide (utilise pour tracer une valeur non resolue).
pub fn looks_like_spec(s: &str) -> bool {
    s.starts_with('@')
}

/// Parse `@root/rel` en `(racine, composants)` — la seule porte
/// d'entree de la grammaire (validation stricte, aucune tolerance
/// de separateur pour eviter les divergences Windows/POSIX).
fn parse_spec(spec: &str) -> Result<(PortableRoot, Vec<String>), PathSpecError> {
    let (tok, rel) = match spec.split_once('/') {
        Some((t, r)) => (t, r),
        None => (spec, ""),
    };
    let root =
        PortableRoot::from_token(tok).ok_or_else(|| PathSpecError::UnknownRoot(tok.to_string()))?;
    let mut parts = Vec::new();
    for comp in rel.split('/') {
        if comp.is_empty() {
            continue;
        }
        if comp == "." || comp == ".." {
            return Err(PathSpecError::Invalid(
                spec.to_string(),
                "composant « . » ou « .. » interdit",
            ));
        }
        if comp.contains('\\') {
            return Err(PathSpecError::Invalid(
                spec.to_string(),
                "separateur « \\ » interdit (utiliser « / »)",
            ));
        }
        if comp.contains(':') {
            return Err(PathSpecError::Invalid(
                spec.to_string(),
                "« : » interdit (lettre de lecteur / flux ADS)",
            ));
        }
        if comp.contains('\0') {
            return Err(PathSpecError::Invalid(
                spec.to_string(),
                "octet NUL interdit",
            ));
        }
        parts.push(comp.to_string());
    }
    Ok((root, parts))
}

/// `path` sous `root` → ses composants relatifs normalises
/// (`None` hors racine ou sur `..` irreductible). Lexical : aucun
/// acces disque, les liens symboliques ne sont pas dereferences —
/// la migration porte sur les chemins **persistes**, pas sur la
/// realite courante du FS.
fn strip(path: &Path, root: &Path) -> Option<Vec<String>> {
    let pn = normalize(path)?;
    let rn = normalize(root)?;
    if pn.len() < rn.len() {
        return None;
    }
    for (a, b) in pn.iter().zip(&rn) {
        let equal = if cfg!(windows) {
            // Le FS Windows est insensible a la casse : `E:` vs `e:`,
            // `Users` vs `users` doivent matcher apres remontage.
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        };
        if !equal {
            return None;
        }
    }
    Some(pn[rn.len()..].to_vec())
}

/// Composants de `path` en forme lexicale normale : `.` absorbes,
/// `..` reduits sur le composant precedent (irreductible → `None`),
/// separateurs/verbatims aplaties en chaines.
fn normalize(path: &Path) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => out.push(p.as_os_str().to_string_lossy().into_owned()),
            Component::RootDir => {
                if out.is_empty() {
                    out.push(String::new());
                }
            }
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                // `..` ne reduit qu'un composant normal — au-dessus
                // d'un prefixe/racine, ou en tete de relatif, il est
                // irreductible (le chemin resterait ambigu).
                Some(last) if !last.is_empty() && !last.ends_with(':') => {
                    out.pop();
                }
                _ => return None,
            },
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> PathRoots {
        PathRoots::from_parts(
            PathBuf::from("/bundle/state"),
            PathBuf::from("/bundle/data/public"),
            PathBuf::from("/bundle/data/private"),
        )
    }

    #[test]
    fn resolve_specs_ok() {
        let r = roots();
        assert_eq!(
            r.resolve("@state/configuration.json").unwrap(),
            PathBuf::from("/bundle/state/configuration.json")
        );
        assert_eq!(
            r.resolve("@public/downloads/film.mkv").unwrap(),
            PathBuf::from("/bundle/data/public/downloads/film.mkv")
        );
        assert_eq!(
            r.resolve("@private").unwrap(),
            PathBuf::from("/bundle/data/private")
        );
        assert_eq!(
            r.resolve("@state/").unwrap(),
            PathBuf::from("/bundle/state")
        );
        // Valeur non-spec : passee telle quelle.
        assert_eq!(
            r.resolve("downloads/legacy").unwrap(),
            PathBuf::from("downloads/legacy")
        );
        assert_eq!(
            r.resolve("C:\\external\\dir").unwrap(),
            PathBuf::from("C:\\external\\dir")
        );
    }

    #[test]
    fn specs_hostiles_refuses() {
        let r = roots();
        // Le refus vrai : aucun spec ne produit de chemin hors racine.
        for bad in [
            "@public/../../etc/passwd",
            "@public/../private/x",
            "@state/./x",
            "@public/a\\b",
            "@public/C:/x",
            "@private/a:b",
            "@nope/x",
            "@/x",
            "@publicx/y",
        ] {
            assert!(r.resolve(bad).is_err(), "spec attendu refuse : {bad}");
        }
        // Composants vides absorbes (`//`, `/` final) — resolution
        // sous la racine, sans fuite possible.
        assert_eq!(
            r.resolve("@public//x").unwrap(),
            PathBuf::from("/bundle/data/public/x")
        );
    }

    #[test]
    fn to_portable_roundtrip() {
        let r = roots();
        let p = Path::new("/bundle/data/public/downloads/film.mkv");
        assert_eq!(
            r.to_portable(p),
            Some("@public/downloads/film.mkv".to_string())
        );
        assert_eq!(
            r.to_portable(Path::new("/bundle/state/rqbit/main")),
            Some("@state/rqbit/main".to_string())
        );
        assert_eq!(
            r.to_portable(Path::new("/bundle/data/private/temp/aa")),
            Some("@private/temp/aa".to_string())
        );
        assert_eq!(
            r.to_portable(Path::new("/bundle/state")),
            Some("@state".into())
        );
        // Hors racines : pas de spec.
        assert_eq!(r.to_portable(Path::new("/home/user/dl")), None);
        // Round-trip complet.
        for p in [
            Path::new("/bundle/data/public/temp/x"),
            Path::new("/bundle/data/private/downloads/y/z"),
        ] {
            let spec = r.to_portable(p).unwrap();
            assert_eq!(r.resolve(&spec).unwrap(), p.to_path_buf());
        }
    }

    #[test]
    fn migrate_valeurs_persistees() {
        let r = roots();
        // Vide / relatif / spec valide : conserves.
        assert_eq!(r.migrate_persisted(""), PathMigration::Keep);
        assert_eq!(r.migrate_persisted("rel/dir"), PathMigration::Keep);
        assert_eq!(
            r.migrate_persisted("@public/downloads/x"),
            PathMigration::Keep
        );
        // Absolu legacy sous state/downloads → zone publique.
        assert_eq!(
            r.migrate_persisted("/bundle/state/downloads/Film"),
            PathMigration::Rewrite("@public/downloads/Film".into())
        );
        // Absolu sous state autre → @state.
        assert_eq!(
            r.migrate_persisted("/bundle/state/rqbit/main/session.json"),
            PathMigration::Rewrite("@state/rqbit/main/session.json".into())
        );
        // Fichier d'identite deplace sous identity/.
        assert_eq!(
            r.migrate_persisted("/bundle/state/identity_seed.bin"),
            PathMigration::Rewrite("@state/identity/identity_seed.bin".into())
        );
        // Absolu hors racines : conserve (choix externe).
        assert_eq!(
            r.migrate_persisted("/home/user/Downloads"),
            PathMigration::External
        );
        // Spec mal forme : signale.
        assert_eq!(r.migrate_persisted("@public/../x"), PathMigration::Invalid);
    }

    #[test]
    fn layout_dev_data_dans_state() {
        // `.onionbit` (nom != "state") : data/ vit DANS le state dir.
        let r = PathRoots::for_state_dir(Path::new("/home/u/.onionbit"));
        assert_eq!(r.public(), Path::new("/home/u/.onionbit/data/public"));
        // La zone doit matcher sa racine propre, pas @state/data/…
        assert_eq!(
            r.to_portable(Path::new("/home/u/.onionbit/data/public/dl")),
            Some("@public/dl".to_string())
        );
    }

    #[test]
    fn layout_portable_data_voisine() {
        let r = PathRoots::for_state_dir(Path::new("/mnt/usb/OnionBit/state"));
        assert_eq!(r.data(), Path::new("/mnt/usb/OnionBit/data"));
        assert_eq!(
            r.public_downloads(),
            Path::new("/mnt/usb/OnionBit/data/public/downloads")
        );
    }

    #[test]
    fn resolve_input_specs_et_chemins() {
        let r = roots();
        assert_eq!(
            r.resolve_input(Path::new("@public/x")).unwrap(),
            PathBuf::from("/bundle/data/public/x")
        );
        assert!(r.resolve_input(Path::new("@public/../x")).is_err());
        assert_eq!(
            r.resolve_input(Path::new("rel/x")).unwrap(),
            PathBuf::from("rel/x")
        );
    }

    #[test]
    fn marqueur_portable_detecte() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bundle");
        let osdir = root.join("windows");
        std::fs::create_dir_all(&osdir).unwrap();
        std::fs::write(root.join(PORTABLE_MARKER), b"").unwrap();
        let exe = osdir.join("onionbit-daemon.exe");
        assert_eq!(find_portable_root(&exe), Some(root.clone()));
        // Sans marqueur : None.
        let noexe = dir.path().join("other").join("x.exe");
        std::fs::create_dir_all(noexe.parent().unwrap()).unwrap();
        assert_eq!(find_portable_root(&noexe), None);
    }
}
