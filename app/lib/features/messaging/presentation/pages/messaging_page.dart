// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/layout/breakpoints.dart';
import '../../../diagnostic/presentation/providers/diagnostic_providers.dart';
import '../../../diagnostic/presentation/widgets/attest_dialog.dart';
import '../../../diagnostic/presentation/widgets/trust_badge.dart';
import '../../domain/messaging_contact.dart';
import '../../domain/messaging_conversation.dart';
import '../providers/messaging_providers.dart';
import '../widgets/conversation_view.dart';
import '../widgets/plane_badge.dart';

/// Page messagerie e2e (ADR-0011) — liste des contacts (états de
/// consentement, demandes `pending` actionnables) + conversation
/// (historique borné + envoi). Tout passe par `/api/messaging/*` :
/// l'UI ne touche jamais le core directement.
class MessagingPage extends ConsumerWidget {
  const MessagingPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    // Garde le pont SSE → invalidation vivant tant que la page est
    // ouverte (les événements re-synchronisent contacts/historique).
    ref.watch(messagingEventsBridgeProvider);
    final enabled = ref.watch(messagingEnabledProvider);

    return enabled.when(
      loading: () => const Center(child: CircularProgressIndicator()),
      error: (e, _) => _MessagePane(
        icon: Icons.error_outline,
        title: l10n.errorMessage('$e'),
      ),
      data: (on) {
        if (!on) return const _DisabledPane();
        final breakpoint = AppBreakpoints.of(
          MediaQuery.sizeOf(context).width,
        );
        if (breakpoint.index >= AppBreakpoint.expanded.index) {
          // ≥ 1024 dp : deux panneaux — conversations/contacts à
          // gauche, fil (rangée d'onglets) à droite (ADR-0021 §5).
          return const AdaptiveListDetail(
            list: _ContactsPane(),
            detail: ConversationTabs(),
            listWidth: 300,
          );
        }
        // compact/medium : la liste OU le fil sélectionné, jamais les
        // deux — le thread pousse en plein écran plutôt qu'en bottom
        // sheet (clavier + défilement des messages y tiennent mal ;
        // patron standard des messageries mobiles).
        return const _NarrowConversation();
      },
    );
  }
}

/// Panneau « messagerie désactivée » — sonde 404 de
/// `messagingEnabledProvider`.
class _DisabledPane extends ConsumerWidget {
  const _DisabledPane();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    return _MessagePane(
      icon: Icons.forum_outlined,
      title: l10n.msgDisabledTitle,
      body: l10n.msgDisabledHint,
    );
  }
}

/// Panneau générique icône + titre + corps (états vides/erreur).
class _MessagePane extends StatelessWidget {
  const _MessagePane({required this.icon, required this.title, this.body});

  final IconData icon;
  final String title;
  final String? body;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(AppSpace.lg),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 48, color: theme.colorScheme.onSurfaceVariant),
            const SizedBox(height: AppSpace.md),
            Text(title, style: theme.textTheme.titleMedium),
            if (body != null) ...[
              const SizedBox(height: AppSpace.sm),
              Text(
                body!,
                style: theme.textTheme.bodyMedium,
                textAlign: TextAlign.center,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// Colonne gauche : adresse locale, demandes `pending` et contacts.
class _ContactsPane extends ConsumerWidget {
  const _ContactsPane();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final stats = ref.watch(messagingStatsProvider).value;
    final pending = ref.watch(messagingPendingProvider).value ?? const [];
    final contacts = ref.watch(messagingContactsProvider).value ?? const [];
    final convs = ref.watch(messagingConversationsProvider).value ?? const [];
    final selectedConv = ref.watch(selectedConversationProvider);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Bandeau adresse locale — copie au clic ; l'explication
        // e2e vit dans un dialogue pour garder la liste lisible.
        if (stats != null)
          ListTile(
            dense: true,
            leading: const Icon(Icons.key_outlined, size: 20),
            title: Text(
              l10n.msgYourAddress,
              style: Theme.of(context).textTheme.labelSmall,
            ),
            subtitle: Text(
              stats.publicKey,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.bodySmall
                  ?.copyWith(fontFamily: AppFontFamilies.mono),
            ),
            trailing: Wrap(
              spacing: 0,
              children: [
                IconButton(
                  icon: const Icon(Icons.copy, size: 18),
                  tooltip: l10n.msgYourAddress,
                  onPressed: () => Clipboard.setData(
                    ClipboardData(text: stats.publicKey),
                  ),
                ),
                IconButton(
                  icon: const Icon(Icons.info_outline, size: 18),
                  tooltip: l10n.msgInfoTitle,
                  onPressed: () => showDialog<void>(
                    context: context,
                    builder: (ctx) => AlertDialog(
                      title: Text(l10n.msgInfoTitle),
                      content: Text(l10n.msgInfo),
                      actions: [
                        TextButton(
                          onPressed: () => Navigator.pop(ctx),
                          child: Text(l10n.cancel),
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        const Divider(height: 1),
        Expanded(
          child: ListView(
            children: [
              // Demandes de consentement en tête — ce sont les seules
              // entrées qui attendent une décision de l'utilisateur.
              if (pending.isNotEmpty) ...[
                _SectionHeader(
                  title: l10n.msgPendingTitle,
                  count: pending.length,
                ),
                for (final c in pending) _PendingTile(contact: c),
              ],
              // Conversations (directes + groupes) — clic = onglet
              // dans le panneau de droite (ADR-0019).
              _SectionHeader(
                title: l10n.msgConversations,
                trailing: IconButton(
                  icon: const Icon(Icons.group_add_outlined, size: 18),
                  tooltip: l10n.msgNewGroup,
                  onPressed: () => _newGroup(context, ref, contacts),
                ),
              ),
              if (convs.isEmpty)
                Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpace.lg,
                    vertical: AppSpace.xs,
                  ),
                  child: Text(
                    l10n.msgNoConversations,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ),
              for (final c in convs)
                _ConvTile(conv: c, selected: c.convId == selectedConv),
              // Carnet de contacts — état de liaison, plans
              // d'adressage (pastilles vert OnionBit / violet Tor)
              // et menu de gestion.
              _SectionHeader(title: l10n.msgContacts),
              if (contacts.isEmpty && pending.isEmpty)
                Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpace.lg,
                    vertical: AppSpace.xs,
                  ),
                  child: Text(
                    l10n.msgNoContacts,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ),
              for (final c in contacts)
                _ContactTile(
                  contact: c,
                  selected: false,
                  onTap: () => _openDirect(ref, c.publicKey),
                ),
              const Divider(height: AppSpace.lg),
              // Découverte : coffre d'amis ext + pairs `msg_v1`
              // suggérés (sections repliables).
              _FriendsVaultSection(
                selfPk: stats?.publicKey ?? '',
                contacts: contacts,
              ),
              _ExtPeersSection(
                contacts: contacts,
                pending: pending,
              ),
            ],
          ),
        ),
        const Divider(height: 1),
        // « Ajouter un contact » + coffre export/import (chiffre
        // pour soi — portable entre devices de meme identite).
        Padding(
          padding: const EdgeInsets.all(AppSpace.sm),
          child: Row(
            children: [
              Expanded(
                child: FilledButton.tonalIcon(
                  icon: const Icon(Icons.person_add_outlined, size: 18),
                  label: Text(l10n.msgAddContact),
                  onPressed: () => _addContact(context, ref),
                ),
              ),
              IconButton(
                icon: const Icon(Icons.lock_open, size: 18),
                tooltip: l10n.msgVaultExport,
                onPressed: () => _vaultExport(context, ref),
              ),
              IconButton(
                icon: const Icon(Icons.file_upload_outlined, size: 18),
                tooltip: l10n.msgVaultImport,
                onPressed: () => _vaultImport(context, ref),
              ),
            ],
          ),
        ),
      ],
    );
  }

  /// Dialogue « ajouter un contact » : clé publique hex →
  /// `connect` (résolution DHT/PEX + `create-e2e`).
  Future<void> _addContact(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final controller = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.msgAddContact),
        content: TextField(
          controller: controller,
          decoration: InputDecoration(
            labelText: l10n.msgPublicKeyLabel,
            border: const OutlineInputBorder(),
          ),
          style: const TextStyle(fontFamily: AppFontFamilies.mono),
          autofocus: true,
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.msgConnect),
          ),
        ],
      ),
    );
    final pk = controller.text.trim();
    controller.dispose();
    if (ok != true || !context.mounted || pk.isEmpty) return;
    try {
      await ref.read(messagingRepositoryProvider).connect(pk);
      await _openDirect(ref, pk);
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
    // Toujours rafraîchir : le contact est créé (et persisté) dès la
    // résolution côté daemon — même si la liaison e2e échoue (contact
    // hors ligne), il doit apparaître dans la liste.
    ref.invalidate(messagingContactsProvider);
    ref.invalidate(messagingHistoryProvider(pk));
    ref.invalidate(messagingConversationsProvider);
  }

  /// Ouvre l'onglet de la conversation directe avec `pk` —
  /// `conv_id` deterministe cote daemon (la conv peut ne pas encore
  /// exister en base : `peer` sert alors le libelle de l'onglet).
  static Future<void> _openDirect(WidgetRef ref, String pk) async {
    try {
      final convId = await ref
          .read(messagingRepositoryProvider)
          .directConversation(pk);
      if (convId.isNotEmpty) {
        ref.read(openConversationsProvider.notifier).open(convId, peer: pk);
      }
    } catch (_) {
      // Messagerie desactivee ou erreur reseau : le SSE re-essaiera.
    }
  }

  /// Dialogue « nouveau groupe » : nom + selection de contacts
  /// actifs → `group_create` (le daemon envoie un `gctl invite` a
  /// chacun — confinement `scope='group'` pour les inconnus).
  Future<void> _newGroup(
    BuildContext context,
    WidgetRef ref,
    List<MessagingContact> contacts,
  ) async {
    final l10n = context.l10n;
    final actives = [
      for (final c in contacts)
        if (c.state == MessagingContactState.active) c,
    ];
    final nameCtl = TextEditingController();
    final chosen = <String>{};
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setState) => AlertDialog(
          title: Text(l10n.msgNewGroup),
          content: SizedBox(
            width: 360,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: nameCtl,
                  decoration: InputDecoration(
                    labelText: l10n.msgGroupNameLabel,
                    border: const OutlineInputBorder(),
                  ),
                  autofocus: true,
                ),
                const SizedBox(height: AppSpace.sm),
                Align(
                  alignment: Alignment.centerLeft,
                  child: Text(
                    l10n.msgGroupPickMembers,
                    style: Theme.of(ctx).textTheme.labelMedium,
                  ),
                ),
                Flexible(
                  child: ListView(
                    shrinkWrap: true,
                    children: [
                      for (final c in actives)
                        CheckboxListTile(
                          dense: true,
                          title: Text(c.displayName),
                          value: chosen.contains(c.publicKey),
                          onChanged: (v) => setState(
                            () => v == true
                                ? chosen.add(c.publicKey)
                                : chosen.remove(c.publicKey),
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: Text(l10n.cancel),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: Text(l10n.msgNewGroup),
            ),
          ],
        ),
      ),
    );
    final name = nameCtl.text.trim();
    nameCtl.dispose();
    if (ok != true || !context.mounted || name.isEmpty) return;
    try {
      final convId = await ref
          .read(messagingRepositoryProvider)
          .groupCreate(name, chosen.toList());
      ref.invalidate(messagingConversationsProvider);
      if (convId.isNotEmpty) {
        ref.read(openConversationsProvider.notifier).open(convId);
      }
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }

  /// Exporte le coffre chiffre (`GET /messaging/vault/export`) —
  /// copie le blob hex dans le presse-papiers.
  Future<void> _vaultExport(BuildContext context, WidgetRef ref) async {
    try {
      final blob = await ref.read(messagingRepositoryProvider).vaultExport();
      await Clipboard.setData(ClipboardData(text: blob));
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(context.l10n.msgVaultExported)),
        );
      }
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }

  /// Importe un blob coffre (`POST /messaging/vault/import`) —
  /// dialogue de collage, puis restaure les contacts inconnus.
  Future<void> _vaultImport(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final controller = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.msgVaultImport),
        content: TextField(
          controller: controller,
          decoration: InputDecoration(
            labelText: l10n.msgVaultPasteHint,
            border: const OutlineInputBorder(),
          ),
          style: const TextStyle(fontFamily: AppFontFamilies.mono, fontSize: 11),
          maxLines: 4,
          autofocus: true,
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.msgVaultImport),
          ),
        ],
      ),
    );
    final blob = controller.text.trim();
    controller.dispose();
    if (ok != true || !context.mounted || blob.isEmpty) return;
    try {
      final n = await ref
          .read(messagingRepositoryProvider)
          .vaultImport(blob);
      ref.invalidate(messagingContactsProvider);
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(l10n.msgVaultRestored(n))),
        );
      }
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }
}

/// En-tête de section du panneau de gauche (Demandes /
/// Conversations / Contacts) — titre discret + compteur
/// optionnel + action de section éventuelle.
class _SectionHeader extends StatelessWidget {
  const _SectionHeader({required this.title, this.count, this.trailing});

  final String title;
  final int? count;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        AppSpace.md,
        AppSpace.md,
        AppSpace.xs,
        AppSpace.xs,
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              count != null ? '$title · $count' : title,
              style: theme.textTheme.labelLarge?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
          ?trailing,
        ],
      ),
    );
  }
}

/// Tuile de conversation (ADR-0019) : icone direct/groupe, libelle,
/// pastille plan (`.onion` associé — ADR-0029), badge de non-lus,
/// indicateur d'invitation, menu suppression.
class _ConvTile extends ConsumerWidget {
  const _ConvTile({required this.conv, required this.selected});

  final MessagingConversation conv;
  final bool selected;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    return ListTile(
      dense: true,
      selected: selected,
      leading: Icon(
        conv.isGroup ? Icons.groups_outlined : Icons.person_outline,
        size: 20,
        color: conv.isInvited ? theme.colorScheme.tertiary : null,
      ),
      title: Text(
        conv.displayName,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: conv.alias.isEmpty && !conv.isGroup && conv.name.isEmpty
            ? const TextStyle(fontFamily: AppFontFamilies.mono, fontSize: 12)
            : null,
      ),
      subtitle: conv.isInvited ? Text(l10n.msgStatePending) : null,
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          // Plan `.onion` disponible sur le correspondant (direct
          // seul — les groupes restent sur les circuits OnionBit).
          if (conv.planes.contains('onion'))
            const Padding(
              padding: EdgeInsets.only(right: AppSpace.xs),
              child: PlaneDot(plane: 'onion', size: 8),
            ),
          if (conv.unread > 0)
            Container(
              padding: const EdgeInsets.symmetric(
                horizontal: 6,
                vertical: 1,
              ),
              decoration: BoxDecoration(
                color: theme.colorScheme.primary,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Text(
                '${conv.unread}',
                style: theme.textTheme.labelSmall
                    ?.copyWith(color: theme.colorScheme.onPrimary),
              ),
            ),
          PopupMenuButton<String>(
            iconSize: 18,
            itemBuilder: (ctx) => [
              PopupMenuItem(
                value: 'delete',
                child: Text(l10n.msgDeleteConversation),
              ),
            ],
            onSelected: (_) => _delete(context, ref),
          ),
        ],
      ),
      onTap: () => ref
          .read(openConversationsProvider.notifier)
          .open(conv.convId, peer: conv.peer),
    );
  }

  Future<void> _delete(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.msgDeleteConversation),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.msgDeleteConversation),
          ),
        ],
      ),
    );
    if (ok != true || !context.mounted) return;
    try {
      await ref.read(messagingRepositoryProvider).convDelete(conv.convId);
      ref.read(openConversationsProvider.notifier).close(conv.convId);
      ref.invalidate(messagingConversationsProvider);
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }
}

/// Tuile de demande `pending` — trois actions consentement.
class _PendingTile extends ConsumerWidget {
  const _PendingTile({required this.contact});

  final MessagingContact contact;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final repo = ref.watch(messagingRepositoryProvider);
    return ListTile(
      dense: true,
      leading: const Icon(Icons.person_add_alt_1_outlined, size: 20),
      title: Row(
        children: [
          Flexible(
            child: Text(
              contact.displayName,
              style: contact.alias.isNotEmpty
                  ? null
                  : const TextStyle(fontFamily: AppFontFamilies.mono),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          // Confiance ext (kind identity) : un inconnu flagué par un
          // curateur suivi est visible avant la décision de
          // consentement.
          TrustBadge(kind: 'identity', subject: contact.publicKey),
        ],
      ),
      subtitle: contact.pendingSinceSecs != null
          ? Text('${contact.pendingSinceSecs} s')
          : null,
      trailing: Wrap(
        spacing: AppSpace.xs,
        children: [
          IconButton(
            icon: const Icon(Icons.check, size: 18),
            tooltip: l10n.msgAccept,
            onPressed: () =>
                _act(context, ref, () => repo.accept(contact.publicKey)),
          ),
          IconButton(
            icon: const Icon(Icons.close, size: 18),
            tooltip: l10n.msgRefuse,
            onPressed: () =>
                _act(context, ref, () => repo.refuse(contact.publicKey)),
          ),
          IconButton(
            icon: const Icon(Icons.block, size: 18),
            tooltip: l10n.msgBlock,
            onPressed: () =>
                _act(context, ref, () => repo.block(contact.publicKey)),
          ),
        ],
      ),
    );
  }
}

/// Tuile contact — avatar avec pastille de liaison e2e (vert lié /
/// orange en cours / rouge échec / gris jamais tenté), pastilles
/// de plans d'adressage (vert OnionBit / violet Tor — ADR-0029),
/// menu reconnexion/blocage/.onion/rétention/suppression.
class _ContactTile extends ConsumerWidget {
  const _ContactTile({
    required this.contact,
    required this.selected,
    required this.onTap,
  });

  final MessagingContact contact;
  final bool selected;
  final VoidCallback onTap;

  /// Pastille de liaison : couleur + libellé de l'état `link`
  /// remonté par le daemon (jamais déduit du seul `circuit_id`).
  (Color, String) _linkVisual(BuildContext context) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    return switch (contact.link) {
      MessagingLinkState.bound => (Colors.green, l10n.msgLinkBound),
      MessagingLinkState.connecting => (
        Colors.orange,
        l10n.msgLinkConnecting,
      ),
      MessagingLinkState.failed => (Colors.red, l10n.msgLinkFailed),
      MessagingLinkState.none => (
        theme.colorScheme.outline,
        l10n.msgLinkOffline,
      ),
    };
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final repo = ref.watch(messagingRepositoryProvider);
    final stateLabel = switch (contact.state) {
      MessagingContactState.pending => l10n.msgStatePending,
      MessagingContactState.blocked => l10n.msgStateBlocked,
      _ => null,
    };
    final (linkColor, linkLabel) = _linkVisual(context);
    return ListTile(
      dense: true,
      selected: selected,
      // Avatar + pastille de liaison e2e en coin (vert lié /
      // orange en cours / rouge échec / gris jamais tenté).
      leading: Tooltip(
        message: linkLabel,
        child: Badge(
          alignment: Alignment.bottomRight,
          smallSize: 9,
          backgroundColor: linkColor,
          child: CircleAvatar(
            radius: 14,
            child: Text(
              contact.displayName.isEmpty
                  ? '?'
                  : contact.displayName[0].toUpperCase(),
              style: const TextStyle(fontSize: 12),
            ),
          ),
        ),
      ),
      title: Row(
        children: [
          Flexible(
            child: Text(
              contact.displayName,
              style: contact.alias.isNotEmpty
                  ? null
                  : const TextStyle(fontFamily: AppFontFamilies.mono),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          TrustBadge(kind: 'identity', subject: contact.publicKey),
          // Plans d'adressage : vert OnionBit toujours, violet Tor
          // quand une adresse `.onion` est associée (tooltip
          // vérifiée/manuelle — pair en ligne requis).
          PlaneDot(
            plane: 'onionbit',
            size: 8,
            tooltip: context.l10n.msgPlaneOnionbit,
          ),
          if (contact.hasOnion)
            PlaneDot(
              plane: 'onion',
              size: 8,
              tooltip: contact.onionVerified
                  ? l10n.msgOnionVerifiedTip
                  : l10n.msgOnionManualTip,
            ),
        ],
      ),
      subtitle: stateLabel != null
          ? Text(
              contact.alias.isNotEmpty
                  ? '${contact.shortKey} · $stateLabel'
                  : stateLabel,
            )
          : contact.alias.isNotEmpty
          ? Text(
              contact.shortKey,
              style: const TextStyle(fontFamily: AppFontFamilies.mono),
            )
          : null,
      trailing: PopupMenuButton<String>(
        iconSize: 18,
        itemBuilder: (ctx) => [
          PopupMenuItem(value: 'rename', child: Text(l10n.msgRename)),
          // « Reconnecter » — relance resolve+liaison e2e sur un
          // contact consenti sans circuit (le connect du daemon est
          // idempotent ; la maintenance retente aussi toute seule).
          if (contact.state == MessagingContactState.active &&
              contact.link != MessagingLinkState.bound)
            PopupMenuItem(value: 'reconnect', child: Text(l10n.msgReconnect)),
          // Association `.onion` explicite (ADR-0029 Q3 — jamais
          // inférée) : adresse saisie à la main ou liaison signée.
          PopupMenuItem(
            value: 'onion',
            child: Text(l10n.msgOnionAssociate),
          ),
          if (contact.state == MessagingContactState.blocked)
            PopupMenuItem(value: 'unblock', child: Text(l10n.msgUnblock))
          else
            PopupMenuItem(value: 'block', child: Text(l10n.msgBlock)),
          PopupMenuItem(
            value: 'retention',
            child: Text(l10n.msgRetentionTitle),
          ),
          PopupMenuItem(value: 'delete', child: Text(l10n.msgDeleteContact)),
          // Attestation `identity` (ADR-0015 §6) — « cet utilisateur
          // est de confiance / nuisible », signée et propagée au mesh.
          const PopupMenuDivider(),
          PopupMenuItem(value: 'endorse', child: Text(l10n.ctxEndorseUser)),
          PopupMenuItem(value: 'flag', child: Text(l10n.ctxFlagUser)),
        ],
        onSelected: (v) => switch (v) {
          'rename' => _renameDialog(context, ref),
          'endorse' => _attest(context, ref, 'endorse'),
          'flag' => _attest(context, ref, 'flag'),
          'reconnect' => _reconnect(context, ref),
          'block' => _act(context, ref, () => repo.block(contact.publicKey)),
          'unblock' => _act(
            context,
            ref,
            () => repo.unblock(contact.publicKey),
          ),
          'retention' => _retentionDialog(context, ref),
          'onion' => _onionDialog(context, ref),
          _ => _act(context, ref, () => repo.remove(contact.publicKey)),
        },
      ),
      onTap: onTap,
    );
  }

  /// Attestation signée sur la clé du contact (`kind=identity`) —
  /// le dialogue est pré-rempli ; en succès on invalide le cache du
  /// score pour que la pastille reflète le verdict immédiatement.
  /// Un `endorse` ajoute aussi la clé à la liste d'amis publique
  /// (auto-attestations récupérables via le gossip sur un autre
  /// device — ADR-0015 §6).
  Future<void> _attest(
    BuildContext context,
    WidgetRef ref,
    String verdict,
  ) async {
    final ok = await AttestDialog.show(
      context,
      kind: 'identity',
      subject: contact.publicKey,
      verdict: verdict,
    );
    if (!ok || !context.mounted) return;
    ref
      ..invalidate(
        extTrustProvider((kind: 'identity', subject: contact.publicKey)),
      )
      ..invalidate(extAttestationsProvider);
  }

  /// « Reconnecter » : nouvelle tentative resolve + liaison e2e.
  /// Le refresh vient du SSE `messaging_link` (connecting → bound /
  /// failed) — on invalide quand même à la fin pour couvrir un flux
  /// SSE absent ou laggué.
  Future<void> _reconnect(BuildContext context, WidgetRef ref) async {
    try {
      await ref
          .read(messagingRepositoryProvider)
          .connect(contact.publicKey);
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
    ref.invalidate(messagingContactsProvider);
  }

  /// Dialogue renommage — pseudonyme local (`''` = effacer, retour
  /// à la clé abrégée).
  Future<void> _renameDialog(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final repo = ref.read(messagingRepositoryProvider);
    final controller = TextEditingController(text: contact.alias);
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.msgRename),
        content: TextField(
          controller: controller,
          decoration: InputDecoration(
            labelText: l10n.msgAliasLabel,
            border: const OutlineInputBorder(),
          ),
          autofocus: true,
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.msgRename),
          ),
        ],
      ),
    );
    final alias = controller.text.trim();
    controller.dispose();
    if (ok != true || !context.mounted) return;
    try {
      await repo.setAlias(contact.publicKey, alias);
      ref.invalidate(messagingContactsProvider);
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }

  /// Dialogue d'association `.onion` (ADR-0029 §8 Q3) : adresse
  /// saisie à la main + signature de liaison optionnelle —
  /// explicite, jamais inférée. Une association existante peut
  /// être retirée (le pair quitte l'allowlist Tor du daemon).
  Future<void> _onionDialog(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final repo = ref.read(messagingRepositoryProvider);
    final addrCtl = TextEditingController(text: contact.onion ?? '');
    final sigCtl = TextEditingController();
    final result = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.msgOnionDialogTitle),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: addrCtl,
              decoration: InputDecoration(
                labelText: l10n.msgOnionAddressLabel,
                border: const OutlineInputBorder(),
              ),
              style: const TextStyle(
                fontFamily: AppFontFamilies.mono,
                fontSize: 12,
              ),
              autofocus: contact.onion == null,
            ),
            const SizedBox(height: AppSpace.sm),
            TextField(
              controller: sigCtl,
              decoration: InputDecoration(
                labelText: l10n.msgOnionSignatureLabel,
                border: const OutlineInputBorder(),
              ),
              style: const TextStyle(
                fontFamily: AppFontFamilies.mono,
                fontSize: 12,
              ),
            ),
            if (contact.hasOnion)
              Padding(
                padding: const EdgeInsets.only(top: AppSpace.sm),
                child: Text(
                  contact.onionVerified
                      ? l10n.msgOnionVerifiedTip
                      : l10n.msgOnionManualTip,
                  style: Theme.of(ctx).textTheme.labelSmall,
                ),
              ),
          ],
        ),
        actions: [
          if (contact.hasOnion)
            TextButton(
              onPressed: () => Navigator.pop(ctx, 'remove'),
              child: Text(l10n.msgOnionRemove),
            ),
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, 'set'),
            child: Text(l10n.msgSend),
          ),
        ],
      ),
    );
    final address = addrCtl.text.trim();
    final signature = sigCtl.text.trim();
    addrCtl.dispose();
    sigCtl.dispose();
    if (result == null || !context.mounted) return;
    try {
      if (result == 'remove') {
        await repo.removePeerOnion(contact.publicKey);
      } else if (address.isNotEmpty) {
        await repo.setPeerOnion(
          contact.publicKey,
          address,
          signature: signature.isEmpty ? null : signature,
        );
      }
      ref.invalidate(messagingContactsProvider);
      ref.invalidate(messagingConversationsProvider);
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }

  /// Dialogue rétention — secondes + suppression sécurisée.
  Future<void> _retentionDialog(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final repo = ref.read(messagingRepositoryProvider);
    final controller = TextEditingController(text: '0');
    var secure = false;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setState) => AlertDialog(
          title: Text(l10n.msgRetentionTitle),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: controller,
                decoration: InputDecoration(
                  labelText: l10n.msgRetentionLabel,
                  border: const OutlineInputBorder(),
                ),
                keyboardType: TextInputType.number,
                autofocus: true,
              ),
              CheckboxListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                title: Text(l10n.msgSecureDelete),
                value: secure,
                onChanged: (v) => setState(() => secure = v ?? false),
              ),
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: Text(l10n.cancel),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: Text(l10n.msgSend),
            ),
          ],
        ),
      ),
    );
    if (ok != true) return;
    try {
      await repo.setRetention(
        contact.publicKey,
        retentionSecs: int.tryParse(controller.text.trim()) ?? 0,
        secureDelete: secure,
      );
      controller.dispose();
    } catch (e) {
      controller.dispose();
      if (context.mounted) _showError(context, e);
    }
  }
}

/// Action dépôt + invalidation commune, erreurs en snackbar.
Future<void> _act(
  BuildContext context,
  WidgetRef ref,
  Future<void> Function() call,
) async {
  try {
    await call();
    ref.invalidate(messagingContactsProvider);
    ref.invalidate(messagingPendingProvider);
    ref.invalidate(messagingConversationsProvider);
  } catch (e) {
    if (context.mounted) _showError(context, e);
  }
}

void _showError(BuildContext context, Object e) {
  final msg = e is ApiException ? e.message : context.l10n.errorMessage('$e');
  ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(msg)));
}

/// « Amis approuvés » — mini coffre-fort de contacts porté par le
/// mesh ext (ADR-0015 §6) : chaque attestation `identity`/`endorse`
/// signée par **notre** clé est une entrée de liste d'amis publique.
/// Sur un nouveau device (même identité ADR-0016), les pairs qui
/// nous suivent re-gossipent nos attestations → les clés reviennent
/// et peuvent être re-ajoutées comme contacts en un clic. La
/// récupération dépend des suiveurs qui ont retenu nos attestations
/// (best-effort — le gossip n'est pas une archive garantie) ; les
/// pseudonymes restent locaux par design (le format signé n'a pas
/// de champ libre).
class _FriendsVaultSection extends ConsumerWidget {
  const _FriendsVaultSection({required this.selfPk, required this.contacts});

  /// Clé publique locale hex (`pk_bin` — comparée à `curator`).
  final String selfPk;
  final List<MessagingContact> contacts;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (selfPk.isEmpty) return const SizedBox.shrink();
    final atts = ref.watch(extAttestationsProvider).value;
    if (atts == null) return const SizedBox.shrink();
    final self = selfPk.toLowerCase();
    final known = contacts
        .map((c) => c.publicKey.toLowerCase())
        .toSet();
    // Mes auto-attestations `endorse` sur des identités, hors
    // contacts déjà enregistrés — le reste est bruit pour ce panneau.
    final friends = [
      for (final a in atts)
        if (a.kind == 'identity' &&
            a.verdict == 'endorse' &&
            a.curator.toLowerCase() == self &&
            !known.contains(a.subject.toLowerCase()))
          a.subject,
    ];
    if (friends.isEmpty) return const SizedBox.shrink();
    final l10n = context.l10n;
    return ExpansionTile(
      dense: true,
      leading: const Icon(Icons.group_outlined, size: 20),
      title: Text(l10n.msgFriendsTitle),
      subtitle: Text('${friends.length}'),
      children: [
        for (final pk in friends)
          ListTile(
            dense: true,
            leading: const Icon(Icons.key, size: 16),
            title: Text(
              pk.length > 16 ? '${pk.substring(0, 16)}…' : pk,
              style: const TextStyle(fontFamily: AppFontFamilies.mono, fontSize: 12),
            ),
            trailing: IconButton(
              icon: const Icon(Icons.person_add_alt_1, size: 18),
              tooltip: l10n.msgFriendRestore,
              onPressed: () async {
                try {
                  await ref.read(messagingRepositoryProvider).connect(pk);
                  ref.invalidate(messagingContactsProvider);
                } catch (e) {
                  if (context.mounted) _showError(context, e);
                }
              },
            ),
          ),
      ],
    );
  }
}

/// Pairs OnionBit découverts avec `msg_v1` et pas encore contacts —
/// suggestions d'ajout sans échange de clé hors-bande (ADR-0015 :
/// le HELLO ext transporte la capacité, le `pk` complet est
/// adressable tel quel).
class _ExtPeersSection extends ConsumerWidget {
  const _ExtPeersSection({required this.contacts, required this.pending});

  final List<MessagingContact> contacts;
  final List<MessagingContact> pending;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final extAsync = ref.watch(extInfoProvider);
    final l10n = context.l10n;
    return extAsync.when(
      loading: () => const SizedBox.shrink(),
      error: (e, _) => const SizedBox.shrink(),
      data: (ext) {
        final known = contacts
            .map((c) => c.publicKey.toLowerCase())
            .followedBy(pending.map((c) => c.publicKey.toLowerCase()))
            .toSet();
        // `msg_v1` annoncé + clé complète dispo + pas encore contact.
        final candidates = [
          for (final p in ext.peers)
            if (p.capsNames.contains('msg_v1') &&
                p.pk.isNotEmpty &&
                !known.contains(p.pk.toLowerCase()))
              p,
        ];
        if (candidates.isEmpty) return const SizedBox.shrink();
        return ExpansionTile(
          dense: true,
          leading: const Icon(Icons.wifi_tethering_outlined, size: 20),
          title: Text(l10n.msgSuggestionsTitle),
          subtitle: Text('${candidates.length}'),
          children: [
            for (final p in candidates)
              ListTile(
                dense: true,
                leading: const Icon(Icons.person_outline, size: 16),
                title: Text(
                  p.mid,
                  style: const TextStyle(
                    fontFamily: AppFontFamilies.mono,
                    fontSize: 12,
                  ),
                ),
                subtitle: Wrap(
                  spacing: 4,
                  children: [
                    for (final c in p.capsNames)
                      Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 5,
                          vertical: 1,
                        ),
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(4),
                          color: const Color(0xFF2E7D32)
                              .withValues(alpha: 0.15),
                        ),
                        child: Text(
                          c,
                          style: const TextStyle(
                            fontSize: 10,
                            color: Color(0xFF2E7D32),
                          ),
                        ),
                      ),
                  ],
                ),
                trailing: IconButton(
                  icon: const Icon(Icons.person_add_alt_1, size: 18),
                  tooltip: l10n.msgAddContact,
                  onPressed: () async {
                    try {
                      await ref
                          .read(messagingRepositoryProvider)
                          .connect(p.pk);
                      await _ContactsPane._openDirect(ref, p.pk);
                      ref.invalidate(messagingContactsProvider);
                    } catch (e) {
                      if (context.mounted) _showError(context, e);
                    }
                  },
                ),
              ),
          ],
        );
      },
    );
  }
}

/// Vue étroite (compact/medium) : la liste des conversations OU le fil
/// sélectionné — jamais les deux. En-tête minimal avec retour à la
/// liste ; la rangée d'onglets de `ConversationTabs` n'a pas de sens
/// sous `expanded` (un seul fil visible à la fois).
class _NarrowConversation extends ConsumerWidget {
  const _NarrowConversation();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final sel = ref.watch(selectedConversationProvider);
    if (sel == null) return const _ContactsPane();
    final tabs = ref.watch(openConversationsProvider);
    final convs =
        ref.watch(messagingConversationsProvider).value ?? const [];
    OpenConvTab tab = (convId: sel, peer: null);
    MessagingConversation? conv;
    for (final t in tabs) {
      if (t.convId == sel) tab = t;
    }
    for (final c in convs) {
      if (c.convId == sel) conv = c;
    }
    final title = conv?.displayName ?? tab.peer ?? tab.convId;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: AppSpace.xs),
          child: Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back),
                tooltip: context.l10n.msgBackToList,
                onPressed: () => ref
                    .read(selectedConversationProvider.notifier)
                    .set(null),
              ),
              Expanded(
                child: Text(
                  title,
                  style: Theme.of(context).textTheme.titleSmall,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        ),
        const Divider(height: 1),
        Expanded(child: ConversationView(tab: tab, conv: conv)),
      ],
    );
  }
}
