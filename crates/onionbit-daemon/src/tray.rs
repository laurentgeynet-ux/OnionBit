// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Icône de zone de notification (systray) du daemon — implémentation
//! Windows.
//!
//! `tray-icon` exige une boucle de messages Win32 sur le thread qui
//! crée l'icône : le tray vit donc sur un thread dédié (la boucle
//! `GetMessage`/`DispatchMessage` pompe aussi les `WM_COMMAND` du menu
//! contextuel — les `MenuEvent` de muda et les `TrayIconEvent` sont
//! poussés dans le channel pendant `DispatchMessage`, on les draine
//! après chaque message). `stop()` poste `WM_QUIT` au thread : la
//! boucle sort, le `TrayIcon` est droppé (l'icône disparaît) puis le
//! thread est joint.
//!
//! Le double-clic gauche ouvre l'UI préférée (`tray/default_ui`,
//! radio du menu, persistant) avec repli sur l'autre UI si elle est
//! indisponible.

use std::path::PathBuf;

use crate::shutdown::ShutdownSignal;

/// UI ouverte au double-clic sur l'icône (`tray/default_ui`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayUi {
    /// Application de bureau (`OnionBit.exe` à côté du daemon).
    App,
    /// Interface web servie par le daemon (navigateur).
    Browser,
}

impl TrayUi {
    /// Parse `tray/default_ui` — inconnu → `App` (défaut sûr).
    pub fn from_config(s: &str) -> Self {
        match s {
            "browser" => Self::Browser,
            _ => Self::App,
        }
    }

    /// Valeur persistée (`tray/default_ui`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Browser => "browser",
        }
    }
}

/// Cible effective du double-clic : l'UI préférée si disponible,
/// sinon l'autre ; `None` si aucune ne l'est (double-clic inerte —
/// jamais d'ouverture aveugle d'une UI absente).
pub fn resolve_ui(preferred: TrayUi, app_available: bool, web_served: bool) -> Option<TrayUi> {
    let available = |ui: TrayUi| match ui {
        TrayUi::App => app_available,
        TrayUi::Browser => web_served,
    };
    if available(preferred) {
        return Some(preferred);
    }
    let other = match preferred {
        TrayUi::App => TrayUi::Browser,
        TrayUi::Browser => TrayUi::App,
    };
    available(other).then_some(other)
}

/// Paramètres de l'icône tray (le type existe sur toutes les
/// plateformes pour que `main` reste sans `cfg`).
#[cfg_attr(not(windows), allow(dead_code))]
pub struct TrayOptions {
    /// Texte au survol (`OnionBit — 127.0.0.1:<port>`).
    pub tooltip: String,
    /// Dossier ouvert par « Ouvrir le dossier des logs ».
    pub logs_dir: PathBuf,
    /// `OnionBit.exe` (ex `onionbit_ui.exe`) à côté du daemon (`None` → item désactivé).
    pub ui_exe: Option<PathBuf>,
    /// Port HTTP réel de l'API (publié après le bind) — lu par
    /// « Ouvrir dans le navigateur » au moment du clic.
    pub api_port: std::sync::Arc<std::sync::atomic::AtomicU16>,
    /// L'UI web est servie par le daemon (`api/web_ui_*` resolu) —
    /// active l'item « Ouvrir dans le navigateur ».
    pub web_ui_served: bool,
    /// UI ouverte au double-clic (`tray/default_ui` — le radio du
    /// menu met à jour la valeur en live et la persiste).
    pub default_ui: TrayUi,
    /// `configuration.json` — destination de la persistance du choix
    /// radio « double-clic » (`tray.default_ui`). `None` = choix
    /// éphémère (jamais d'écriture).
    pub config_path: Option<PathBuf>,
    /// Sonde de session remplie par `main` après `CoreSession::start`
    /// — lue en live : en session invitée (ADR-0022, zéro artefact)
    /// le choix radio n'est jamais persisté.
    pub session: std::sync::Arc<std::sync::Mutex<Option<onionbit_core::CoreSession>>>,
    /// Ligne de commande écrite dans la clé Run (autostart).
    pub autostart_cmd: String,
    /// Couleur d'icône personnalisée (`tray_icon_color` Python —
    /// recolor de l'icône ; `None` = ressource `IDI_ICON` /
    /// `onionbit.ico` / carré bleu par défaut). Quand elle est fournie,
    /// un carré RGB de cette couleur remplace l'icône — on ne sait
    /// pas recolorer une ressource `.ico`.
    pub icon_color: Option<[u8; 3]>,
    /// Signal déclenché par « Quitter ».
    pub shutdown: ShutdownSignal,
    /// Mises à jour de tooltip (ex. port réel une fois l'API bindée —
    /// l'icône est créée avant `CoreSession::start`, comme le GUI
    /// Python qui s'affiche avant que le core soit prêt).
    pub tooltip_rx: Option<std::sync::mpsc::Receiver<String>>,
}

/// Handle runtime du tray (`None` si désactivé ou échec de création).
pub struct TrayHandle {
    #[cfg(windows)]
    inner: windows_impl::Inner,
}

#[cfg(windows)]
const WM_TRAY: u32 = 0x8000; // WM_APP : reveille la pompe pour les updates.

#[cfg(windows)]
mod windows_impl {
    use std::process::Command;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::{Icon, MouseButton, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_QUIT,
    };

    use super::{resolve_ui, TrayOptions, TrayUi};
    use crate::autostart;

    /// Identifiant de la ressource icône (cf. `resources.rc`).
    const IDI_ICON: u16 = 101;

    /// Libellés du menu — le tray vit dans le daemon (pas d'ARB
    /// Flutter) : table fr/en choisie sur la locale UI Windows.
    struct Labels {
        open_app: &'static str,
        open_browser: &'static str,
        dbl_click: &'static str,
        ui_app: &'static str,
        ui_browser: &'static str,
        autostart: &'static str,
        open_logs: &'static str,
        quit: &'static str,
    }

    const FR: Labels = Labels {
        open_app: "Ouvrir OnionBit — application de bureau",
        open_browser: "Ouvrir OnionBit — interface web (navigateur)",
        dbl_click: "Au double-clic, ouvrir",
        ui_app: "Application de bureau",
        ui_browser: "Interface web (navigateur)",
        autostart: "Démarrer OnionBit avec Windows",
        open_logs: "Ouvrir le dossier des logs",
        quit: "Quitter OnionBit",
    };

    const EN: Labels = Labels {
        open_app: "Open OnionBit — desktop app",
        open_browser: "Open OnionBit — web interface (browser)",
        dbl_click: "On double-click, open",
        ui_app: "Desktop app",
        ui_browser: "Web interface (browser)",
        autostart: "Start OnionBit with Windows",
        open_logs: "Open logs folder",
        quit: "Quit OnionBit",
    };

    /// Locale de l'UI Windows (`PRIMARYLANGID` == `LANG_FRENCH`).
    fn labels() -> &'static Labels {
        let lang = unsafe { GetUserDefaultUILanguage() };
        if lang & 0x3FF == 0x0C {
            &FR
        } else {
            &EN
        }
    }

    pub struct Inner {
        thread_id: u32,
        tooltip_tx: mpsc::Sender<String>,
        join: std::thread::JoinHandle<()>,
    }

    impl Inner {
        /// `WM_QUIT` termine la pompe à messages ; le drop du `TrayIcon`
        /// retire l'icône de la zone de notification, puis on joint.
        pub fn stop(self) {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
            }
            let _ = self.join.join();
        }

        /// Met à jour le texte de survol (port réel une fois bindé).
        pub fn set_tooltip(&self, tooltip: String) {
            if self.tooltip_tx.send(tooltip).is_ok() {
                unsafe {
                    let _ = PostThreadMessageW(self.thread_id, super::WM_TRAY, 0, 0);
                }
            }
        }
    }

    /// Crée le thread tray. `None` si le thread ou l'icône n'a pas pu
    /// être créée (le daemon continue sans systray).
    pub fn spawn(mut opts: TrayOptions) -> Option<Inner> {
        let (tx, rx) = mpsc::channel();
        let (ttx, trx) = mpsc::channel();
        opts.tooltip_rx = Some(trx);
        let join = std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || run(opts, tx))
            .map_err(|e| {
                tracing::warn!(error = %e, "thread systray impossible à créer");
                e
            })
            .ok()?;
        // Le thread répond Some(tid) si l'icône est créée, None sinon.
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Some(thread_id)) => Some(Inner {
                thread_id,
                tooltip_tx: ttx,
                join,
            }),
            Ok(None) => {
                let _ = join.join();
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "thread systray muet");
                None
            }
        }
    }

    fn run(opts: TrayOptions, ready: mpsc::Sender<Option<u32>>) {
        let thread_id = unsafe { GetCurrentThreadId() };
        let l = labels();
        // UI préférée du double-clic — vivante : le radio du menu la
        // fait basculer sans redémarrage (0 = App, 1 = Browser).
        let current_ui = AtomicU8::new(match opts.default_ui {
            TrayUi::App => 0,
            TrayUi::Browser => 1,
        });

        let open_ui = MenuItem::new(l.open_app, opts.ui_exe.is_some(), None);
        let open_browser = MenuItem::new(l.open_browser, opts.web_ui_served, None);
        let dbl_app = CheckMenuItem::new(l.ui_app, true, opts.default_ui == TrayUi::App, None);
        let dbl_browser = CheckMenuItem::new(
            l.ui_browser,
            opts.web_ui_served,
            opts.default_ui == TrayUi::Browser,
            None,
        );
        let dbl_menu = Submenu::new(l.dbl_click, true);
        let autostart_item = CheckMenuItem::new(l.autostart, true, autostart::is_enabled(), None);
        let open_logs = MenuItem::new(l.open_logs, true, None);
        let quit = MenuItem::new(l.quit, true, None);

        let menu = Menu::new();
        let built = dbl_menu
            .append_items(&[&dbl_app, &dbl_browser])
            .and_then(|_| {
                menu.append_items(&[
                    &open_ui,
                    &open_browser,
                    &PredefinedMenuItem::separator(),
                    &dbl_menu,
                    &autostart_item,
                    &open_logs,
                    &PredefinedMenuItem::separator(),
                    &quit,
                ])
            });
        if built.is_err() {
            tracing::warn!("menu systray impossible à construire");
            let _ = ready.send(None);
            return;
        }

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(&opts.tooltip)
            .with_icon(load_icon(opts.icon_color))
            .build();
        let tray = match tray {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "icône systray impossible à créer");
                let _ = ready.send(None);
                return;
            }
        };
        let _ = ready.send(Some(thread_id));
        tracing::info!("icône de notification créée");

        // Pompe Win32 : les WM_COMMAND du menu et les clics sur
        // l'icône arrivent pendant DispatchMessageW — drainer les
        // MenuEvent, les TrayIconEvent et les updates de tooltip
        // juste après (WM_TRAY reveille la pompe).
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                drain_menu_events(
                    &opts,
                    &current_ui,
                    &open_ui,
                    &open_browser,
                    &dbl_app,
                    &dbl_browser,
                    &autostart_item,
                    &open_logs,
                    &quit,
                );
                drain_tray_events(&opts, &current_ui);
                if let Some(rx) = &opts.tooltip_rx {
                    while let Ok(tip) = rx.try_recv() {
                        let _ = tray.set_tooltip(Some(&tip));
                    }
                }
            }
        }
        tracing::info!("thread systray terminé");
    }

    /// Consomme tous les événements menu en attente.
    #[allow(clippy::too_many_arguments)]
    fn drain_menu_events(
        opts: &TrayOptions,
        current_ui: &AtomicU8,
        open_ui: &MenuItem,
        open_browser: &MenuItem,
        dbl_app: &CheckMenuItem,
        dbl_browser: &CheckMenuItem,
        autostart_item: &CheckMenuItem,
        open_logs: &MenuItem,
        quit: &MenuItem,
    ) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = &event.id;
            if id == quit.id() {
                tracing::info!("arrêt demandé via le systray");
                opts.shutdown.trigger();
            } else if id == open_logs.id() {
                let _ = Command::new("explorer").arg(&opts.logs_dir).spawn();
            } else if id == open_ui.id() {
                open_desktop_app(opts);
            } else if id == open_browser.id() {
                open_web_ui(opts);
            } else if id == dbl_app.id() || id == dbl_browser.id() {
                // Radio émulé : un seul coché ; le choix est persisté
                // (`tray.default_ui`) et lu en live par le double-clic.
                let ui = if id == dbl_browser.id() {
                    TrayUi::Browser
                } else {
                    TrayUi::App
                };
                current_ui.store(ui as u8, Ordering::Relaxed);
                dbl_app.set_checked(ui == TrayUi::App);
                dbl_browser.set_checked(ui == TrayUi::Browser);
                persist_default_ui(opts, ui);
            } else if id == autostart_item.id() {
                match autostart::toggle(&opts.autostart_cmd) {
                    Ok(on) => autostart_item.set_checked(on),
                    Err(e) => {
                        tracing::warn!(error = %e, "bascule de l'autostart impossible");
                    }
                }
            }
        }
    }

    /// Double-clic gauche sur l'icône → ouvre l'UI préférée (repli
    /// sur l'autre si elle est indisponible).
    fn drain_tray_events(opts: &TrayOptions, current_ui: &AtomicU8) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                let preferred = if current_ui.load(Ordering::Relaxed) == 1 {
                    TrayUi::Browser
                } else {
                    TrayUi::App
                };
                match resolve_ui(preferred, opts.ui_exe.is_some(), opts.web_ui_served) {
                    Some(TrayUi::App) => open_desktop_app(opts),
                    Some(TrayUi::Browser) => open_web_ui(opts),
                    None => {}
                }
            }
        }
    }

    /// Lance l'application de bureau (`OnionBit.exe`).
    fn open_desktop_app(opts: &TrayOptions) {
        if let Some(exe) = &opts.ui_exe {
            if let Err(e) = Command::new(exe).spawn() {
                tracing::warn!(error = %e, "lancement de l'UI impossible");
            }
        }
    }

    /// UI web servie par le daemon : navigateur par défaut sur
    /// l'origine loopback (clé API demandée par l'UI).
    fn open_web_ui(opts: &TrayOptions) {
        let port = opts.api_port.load(Ordering::Relaxed);
        if port > 0 {
            let url = format!("http://127.0.0.1:{port}/");
            if let Err(e) = Command::new("cmd").args(["/c", "start", "", &url]).spawn() {
                tracing::warn!(error = %e, "ouverture du navigateur impossible");
            }
        }
    }

    /// Persiste `tray.default_ui` dans `configuration.json`
    /// (load → set → write sparse, comme l'API settings) — jamais en
    /// session invitée (zéro artefact, ADR-0022). `is_guest` est lu
    /// en live : la transition invité peut survenir après le boot
    /// (gate `identity_pending` → session invitée via l'UI).
    fn persist_default_ui(opts: &TrayOptions, ui: TrayUi) {
        let guest = opts
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|s| s.is_guest());
        if guest {
            return;
        }
        let Some(path) = &opts.config_path else {
            return;
        };
        let mut cfg = onionbit_core::DaemonConfig::load(path);
        cfg.tray.default_ui = ui.as_str().to_string();
        if let Err(e) = cfg.write(path) {
            tracing::warn!(error = %e, "tray.default_ui : persistance impossible");
        }
    }

    /// Icône du tray : `tray_icon_color` (carré RGB recoloré, comme le
    /// `recolor_tray_icon` Python) → ressource embarquée `IDI_ICON`
    /// (build.rs) → `onionbit.ico` à côté de l'exe → carré bleu généré
    /// (dernier recours pour que l'entrée systray existe toujours).
    fn load_icon(icon_color: Option<[u8; 3]>) -> Icon {
        if let Some([r, g, b]) = icon_color {
            let rgba = vec![[r, g, b, 0xFF]; 32 * 32].concat();
            return Icon::from_rgba(rgba, 32, 32).expect("icone RGBA 32×32 toujours valide");
        }
        if let Ok(icon) = Icon::from_resource(IDI_ICON, Some((32, 32))) {
            return icon;
        }
        if let Some(path) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.join("onionbit.ico")))
            .filter(|p| p.exists())
        {
            if let Ok(icon) = Icon::from_path(&path, Some((32, 32))) {
                return icon;
            }
        }
        // 32×32 BGRA opaque (bleu Tribler #2F6FED — accent du thème UI).
        let rgba = vec![[0x2F, 0x6F, 0xED, 0xFF]; 32 * 32].concat();
        Icon::from_rgba(rgba, 32, 32).expect("icone RGBA 32×32 toujours valide")
    }
}

#[cfg(windows)]
pub fn spawn(opts: TrayOptions) -> Option<TrayHandle> {
    windows_impl::spawn(opts).map(|inner| TrayHandle { inner })
}

/// Hors Windows : pas de systray pour l'instant (Linux/macOS feront
/// l'objet d'une étape dédiée — AppIndicator/gtk ou NSStatusItem).
#[cfg(not(windows))]
pub fn spawn(_opts: TrayOptions) -> Option<TrayHandle> {
    None
}

impl TrayHandle {
    /// Met à jour le texte de survol (no-op hors Windows).
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn set_tooltip(&self, tooltip: String) {
        #[cfg(windows)]
        self.inner.set_tooltip(tooltip);
    }

    /// Retire l'icône et termine le thread tray.
    pub fn stop(self) {
        #[cfg(windows)]
        self.inner.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_ui_parse_et_valeur_persistee() {
        assert_eq!(TrayUi::from_config("app"), TrayUi::App);
        assert_eq!(TrayUi::from_config("browser"), TrayUi::Browser);
        assert_eq!(TrayUi::from_config("nimp"), TrayUi::App);
        assert_eq!(TrayUi::App.as_str(), "app");
        assert_eq!(TrayUi::Browser.as_str(), "browser");
    }

    /// Double-clic : préférée si dispo, repli sur l'autre, inerte si
    /// aucune (jamais d'ouverture d'une UI absente).
    #[test]
    fn resolve_ui_prefere_puis_replie() {
        assert_eq!(resolve_ui(TrayUi::App, true, true), Some(TrayUi::App));
        assert_eq!(
            resolve_ui(TrayUi::Browser, true, true),
            Some(TrayUi::Browser)
        );
        // Préférée indisponible → repli.
        assert_eq!(resolve_ui(TrayUi::Browser, true, false), Some(TrayUi::App));
        assert_eq!(resolve_ui(TrayUi::App, false, true), Some(TrayUi::Browser));
        // Aucune UI → inerte.
        assert_eq!(resolve_ui(TrayUi::App, false, false), None);
    }
}
