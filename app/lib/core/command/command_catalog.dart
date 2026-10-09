// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../features/downloads/presentation/widgets/add_download_dialog.dart';
import '../../features/messaging/presentation/providers/messaging_providers.dart';
import '../../features/settings/presentation/settings_catalog.dart';
import '../l10n/l10n_ext.dart';
import '../router/nav_catalog.dart';
import 'app_command.dart';

export 'app_command.dart';

/// Construit le catalogue de commandes pour le contexte courant
/// (ADR-0021 §6) — destinations `kNavCatalog`, conversations du
/// provider messagerie, sections de réglages `kSettingsSections` et
/// actions rapides.
///
/// Fonction plutôt que provider : les titres sont localisés
/// (`context.l10n` requis) et la palette reconstruit la liste à
/// chaque ouverture / frappe.
List<AppCommand> buildCommandCatalog(BuildContext context, WidgetRef ref) {
  final l10n = context.l10n;
  final commands = <AppCommand>[];

  // Destinations top-level — même source de vérité que la sidebar.
  for (final d in kNavCatalog) {
    commands.add(
      AppCommand(
        id: 'nav.${d.id.name}',
        group: CommandGroup.navigation,
        icon: d.icon,
        title: d.label(l10n),
        keywords: d.path,
        run: (ctx, _) => ctx.go(d.path),
      ),
    );
  }

  // Conversations — ouvre/sélectionne l'onglet puis route ; les non-lus
  // apparaissent en sous-titre pour prioriser à l'œil (pas au filtre).
  final convs =
      ref.watch(messagingConversationsProvider).value ?? const [];
  for (final c in convs) {
    commands.add(
      AppCommand(
        id: 'conv.${c.convId}',
        group: CommandGroup.messaging,
        icon: c.isGroup ? Icons.group_outlined : Icons.chat_bubble_outline,
        title: c.displayName,
        subtitle: c.unread > 0 ? l10n.cmdUnreadCount(c.unread) : null,
        keywords: c.peer ?? '',
        run: (ctx, ref) {
          ref
              .read(openConversationsProvider.notifier)
              .open(c.convId, peer: c.peer);
          ctx.go('/messages');
        },
      ),
    );
  }

  // Sections de réglages — deep-link `/settings?s=<id>`, généralisation
  // du mécanisme d'ancres `settingsSectionKeywords` (ADR-0021 §6).
  for (final s in SettingsSectionId.values) {
    commands.add(
      AppCommand(
        id: 'settings.${s.name}',
        group: CommandGroup.settings,
        icon: s.icon,
        title: s.title(l10n),
        subtitle: l10n.navSettings,
        keywords: settingsSectionKeywords[s] ?? '',
        run: (ctx, _) => ctx.go('/settings?s=${s.name}'),
      ),
    );
  }

  // Actions rapides.
  commands.add(
    AppCommand(
      id: 'action.addDownload',
      group: CommandGroup.actions,
      icon: Icons.add_link,
      title: l10n.cmdAddDownload,
      keywords: 'torrent magnet ajouter add telecharger download',
      run: (ctx, _) => AddDownloadDialog.show(ctx),
    ),
  );

  return commands;
}
