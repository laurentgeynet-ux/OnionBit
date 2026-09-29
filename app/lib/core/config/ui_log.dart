/// Journal UI partagé (`state/logs/ui.log`) — horodaté, affiché dans
/// Diagnostic → Journaux → « Journal UI ».
///
/// Points instrumentés : découverte/spawn du daemon, bascule SSE
/// (connexion/coupure/retries), re-découverte de la configuration,
/// ajouts de téléchargement (magnet surtout) et recherche distante.
/// Sur web : no-op (pas de `dart:io`).
library;

import 'ui_log_stub.dart' if (dart.library.io) 'ui_log_native.dart'
    as impl;

/// Ajoute une ligne horodatée au journal UI (jamais bloquant).
void uiLog(String msg) => impl.uiLog(msg);

/// Lit le journal UI (300 dernières lignes) — vide hors desktop.
Future<String> readUiLog() => impl.readUiLog();
