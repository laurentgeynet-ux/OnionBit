// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Icône de zone de notification (systray) du daemon — implémentation
//! Windows.
//!
//! `tray-icon` exige une boucle de messages Win32 sur le thread qui
//! crée l'icône : le tray vit donc sur un thread dédié (la boucle
//! `GetMessage`/`DispatchMessage` pompe aussi les `WM_COMMAND` du menu
//! contextuel — les `MenuEvent` de muda sont poussés dans le channel
//! pendant `DispatchMessage`, on les draine après chaque message).
//! `stop()` poste `WM_QUIT` au thread : la boucle sort, le `TrayIcon`
//! est droppé (l'icône disparaît) puis le thread est joint.

use std::path::PathBuf;

use crate::shutdown::ShutdownSignal;

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
    use std::sync::mpsc;
    use std::time::Duration;

    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_QUIT,
    };

    use super::TrayOptions;
    use crate::autostart;

    /// Identifiant de la ressource icône (cf. `resources.rc`).
    const IDI_ICON: u16 = 101;

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

        let open_ui = MenuItem::new("Ouvrir OnionBit", opts.ui_exe.is_some(), None);
        let open_browser = MenuItem::new("Ouvrir dans le navigateur", opts.web_ui_served, None);
        let autostart_item =
            CheckMenuItem::new("Démarrer avec Windows", true, autostart::is_enabled(), None);
        let open_logs = MenuItem::new("Ouvrir le dossier des logs", true, None);
        let quit = MenuItem::new("Quitter", true, None);

        let menu = Menu::new();
        if menu
            .append_items(&[
                &open_ui,
                &open_browser,
                &autostart_item,
                &open_logs,
                &PredefinedMenuItem::separator(),
                &quit,
            ])
            .is_err()
        {
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

        // Pompe Win32 : les WM_COMMAND du menu arrivent pendant
        // DispatchMessageW — drainer les MenuEvent et les updates de
        // tooltip juste après (WM_TRAY reveille la pompe).
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                drain_menu_events(
                    &opts,
                    &open_ui,
                    &open_browser,
                    &autostart_item,
                    &open_logs,
                    &quit,
                );
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
    fn drain_menu_events(
        opts: &TrayOptions,
        open_ui: &MenuItem,
        open_browser: &MenuItem,
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
                if let Some(exe) = &opts.ui_exe {
                    if let Err(e) = Command::new(exe).spawn() {
                        tracing::warn!(error = %e, "lancement de l'UI impossible");
                    }
                }
            } else if id == open_browser.id() {
                // UI web servie par le daemon : navigateur par défaut
                // sur l'origine loopback (clé API demandée par l'UI).
                let port = opts.api_port.load(std::sync::atomic::Ordering::Relaxed);
                if port > 0 {
                    let url = format!("http://127.0.0.1:{port}/");
                    if let Err(e) = Command::new("cmd").args(["/c", "start", "", &url]).spawn() {
                        tracing::warn!(error = %e, "ouverture du navigateur impossible");
                    }
                }
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
