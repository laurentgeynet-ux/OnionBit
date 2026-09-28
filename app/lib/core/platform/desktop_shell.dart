/// Initialisation desktop (fenêtre), spécifique plateforme.
///
/// Sur desktop (`dart:io`) : `windowManager.ensureInitialized()` +
/// taille minimale de fenêtre. Sur web : no-op — `window_manager`
/// utilise `dart:ffi` et ne compile pas pour le web.
library;

import 'desktop_shell_stub.dart'
    if (dart.library.io) 'desktop_shell_native.dart'
    as impl;

/// Initialise le shell desktop (fenêtre). No-op sur web/mobile.
Future<void> initDesktopShell() => impl.initDesktopShell();

/// Ouvre un chemin dans l'explorateur de fichiers natif.
Future<void> openPath(String path) => impl.openPath(path);
