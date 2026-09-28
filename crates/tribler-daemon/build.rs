//! `build.rs` de `tribler-daemon` : embarque `resources.rc`
//! (icone `tribler.ico` + infos de version) dans l'exe Windows.
//! Hors cible Windows : no-op (`CARGO_CFG_WINDOWS` non defini).
//! Un echec de compilation de ressource degrade en warning (l'icone
//! tray a un fallback `Icon::from_rgba`), jamais en erreur de build.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        match embed_resource::compile("resources.rc", embed_resource::NONE) {
            embed_resource::CompilationResult::Ok
            | embed_resource::CompilationResult::NotWindows => {}
            other => println!("cargo:warning=resources.rc non embarquée : {other}"),
        }
    }
}
