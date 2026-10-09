// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:go_router/go_router.dart';

import '../../features/downloads/presentation/widgets/add_download_dialog.dart';
import 'app_intents.dart';

/// Raccourcis clavier transverses de la coquille (ADR-0021 §6) —
/// généralisent le patron des `CallbackShortcuts` jusque-là isolés
/// dans `downloads_page.dart` (Ctrl+A/Échap/Espace/Suppr/F2, qui
/// restent locaux à la sélection de la table). Ici : navigation/
/// actions globales, valables depuis n'importe quel écran de la
/// coquille.
///
/// Aucune détection de plateforme : un clavier physique est requis
/// pour déclencher ces combinaisons, donc elles sont simplement
/// inatteignables au clavier virtuel tactile — inoffensif ailleurs.
/// `meta` (Cmd) et `control` (Ctrl) sont mappés sur la même action
/// pour couvrir macOS et Windows/Linux sans détection de plateforme.
class AppShortcuts extends StatelessWidget {
  const AppShortcuts({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Shortcuts(
      shortcuts: const {
        SingleActivator(LogicalKeyboardKey.keyF, control: true):
            GoToSearchIntent(),
        SingleActivator(LogicalKeyboardKey.keyF, meta: true):
            GoToSearchIntent(),
        SingleActivator(LogicalKeyboardKey.keyN, control: true):
            AddDownloadIntent(),
        SingleActivator(LogicalKeyboardKey.keyN, meta: true):
            AddDownloadIntent(),
      },
      child: Actions(
        actions: {
          GoToSearchIntent: CallbackAction<GoToSearchIntent>(
            onInvoke: (intent) {
              context.go('/search');
              return null;
            },
          ),
          AddDownloadIntent: CallbackAction<AddDownloadIntent>(
            onInvoke: (intent) {
              AddDownloadDialog.show(context);
              return null;
            },
          ),
        },
        child: child,
      ),
    );
  }
}
