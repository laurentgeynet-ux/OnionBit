// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Lancement du navigateur `.onion` (`POST /api/browser/launch`,
/// ADR-0030 §6) — point d'entrée unique pour l'UI : le daemon
/// re-valide schéma, cible, politique de sortie et état Tor ; le
/// code machine `error.code` est traduit, jamais le message brut.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../core/api/api_client.dart' show ApiException;
import '../../core/l10n/l10n_ext.dart';
import '../../l10n/app_localizations.dart' show AppLocalizations;
import '../diagnostic/presentation/providers/diagnostic_providers.dart';

/// Lance le navigateur sur `url` — retourne `true` quand le daemon
/// accepte (pid rendu), snackbar honnête sinon.
Future<bool> launchOnionBrowser(
  BuildContext context,
  WidgetRef ref,
  String url,
) async {
  final l10n = context.l10n;
  final messenger = ScaffoldMessenger.of(context);
  try {
    final r = await ref
        .read(diagnosticRepositoryProvider)
        .launchBrowser(url);
    messenger.showSnackBar(
      SnackBar(content: Text(l10n.browserLaunched(r.pid))),
    );
    return true;
  } on ApiException catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text(browserErrorLabel(l10n, e))),
    );
  } catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text(l10n.browserErrGeneric('$e'))),
    );
  }
  return false;
}

/// Lance le navigateur sur `http://<hôte>/` — forme courte pour
/// les hôtes nus (allowlist, contacts `.onion`).
Future<bool> launchOnionHost(
  BuildContext context,
  WidgetRef ref,
  String host,
) =>
    launchOnionBrowser(context, ref, 'http://$host/');

/// Variante embarquée (`embed: true`, Windows) — rend le handle
/// natif de la fenêtre navigateur à reparenter, `null` si le
/// navigateur ne l'a pas publié (repli fenêtre externe, snackbar
/// « lancé » affiché) ou en cas d'échec (snackbar d'erreur).
Future<int?> launchOnionBrowserEmbedded(
  BuildContext context,
  WidgetRef ref,
  String url,
) async {
  final l10n = context.l10n;
  final messenger = ScaffoldMessenger.of(context);
  try {
    final r = await ref
        .read(diagnosticRepositoryProvider)
        .launchBrowser(url, embed: true);
    if (r.hwnd == null) {
      messenger.showSnackBar(
        SnackBar(content: Text(l10n.browserLaunched(r.pid))),
      );
    }
    return r.hwnd;
  } on ApiException catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text(browserErrorLabel(l10n, e))),
    );
  } catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text(l10n.browserErrGeneric('$e'))),
    );
  }
  return null;
}

/// Libellé localisé d'un échec `POST /api/browser/launch` — le code
/// machine (`error.code`, ADR-0030 §6) prime sur le message libre.
String browserErrorLabel(AppLocalizations l10n, ApiException e) =>
    switch (e.code) {
      'invalid_url' => l10n.browserErrInvalidUrl,
      'denied_target' => l10n.browserErrDenied,
      'browser_not_configured' => l10n.browserErrNotConfigured,
      'tor_disabled' => l10n.browserErrTorDisabled,
      'tor_bootstrapping' => l10n.browserErrTorBootstrapping,
      'tor_not_ready' => l10n.browserErrTorNotReady,
      'bootstrap_failed' => l10n.browserErrBootstrapFailed,
      _ => l10n.browserErrGeneric(e.message),
    };
