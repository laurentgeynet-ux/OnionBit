// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/layout/drop_detector.dart';
import '../../../../core/platform/pick_file.dart';
import '../../../../core/design/design_tokens.dart';
import '../../domain/messaging_attachment.dart';
import '../../domain/messaging_conversation.dart';
import '../../domain/messaging_message.dart';
import '../providers/messaging_providers.dart';

/// Panneau droit ADR-0019 : rangée d'onglets de conversations
/// ouvertes + vue de la conversation sélectionnée.
class ConversationTabs extends ConsumerWidget {
  const ConversationTabs({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final tabs = ref.watch(openConversationsProvider);
    final selected = ref.watch(selectedConversationProvider);
    final convs = ref.watch(messagingConversationsProvider).value ?? const [];

    if (tabs.isEmpty) {
      return _EmptyPane(
        icon: Icons.chat_bubble_outline,
        title: l10n.msgSelectConversation,
      );
    }
    final tab = tabs.firstWhere(
      (t) => t.convId == selected,
      orElse: () => tabs.first,
    );
    MessagingConversation? conv;
    for (final c in convs) {
      if (c.convId == tab.convId) {
        conv = c;
        break;
      }
    }
    return Column(
      children: [
        // Rangée d'onglets (UI-only — jamais persistés).
        SizedBox(
          height: 40,
          child: ListView(
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.symmetric(horizontal: AppSpace.sm),
            children: [
              for (final t in tabs)
                _TabChip(
                  tab: t,
                  conv: _find(convs, t.convId),
                  selected: t.convId == tab.convId,
                ),
            ],
          ),
        ),
        const Divider(height: 1),
        Expanded(child: ConversationView(tab: tab, conv: conv)),
      ],
    );
  }

  static MessagingConversation? _find(
    List<MessagingConversation> convs,
    String convId,
  ) {
    for (final c in convs) {
      if (c.convId == convId) return c;
    }
    return null;
  }
}

/// Onglet d'une conversation — libellé + croix de fermeture.
class _TabChip extends ConsumerWidget {
  const _TabChip({required this.tab, required this.conv, required this.selected});

  final OpenConvTab tab;
  final MessagingConversation? conv;
  final bool selected;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final label = conv?.displayName ??
        _shortPeer(ref, tab.peer) ??
        (tab.convId.length > 12 ? '${tab.convId.substring(0, 12)}…' : tab.convId);
    final unread = conv?.unread ?? 0;
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: 2,
        vertical: AppSpace.xs,
      ),
      child: InputChip(
        selected: selected,
        avatar: Icon(
          conv?.isGroup == true ? Icons.groups_outlined : Icons.person_outline,
          size: 16,
        ),
        label: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(label),
            if (unread > 0) ...[
              const SizedBox(width: AppSpace.xs),
              _UnreadBadge(count: unread),
            ],
          ],
        ),
        onPressed: () => ref
            .read(selectedConversationProvider.notifier)
            .set(tab.convId),
        onDeleted: () =>
            ref.read(openConversationsProvider.notifier).close(tab.convId),
      ),
    );
  }

  /// Alias du correspondant quand la conv n'est pas encore persistée
  /// (résolu via la liste de contacts).
  String? _shortPeer(WidgetRef ref, String? peer) {
    if (peer == null) return null;
    final contacts = ref.watch(messagingContactsProvider).value ?? const [];
    for (final c in contacts) {
      if (c.publicKey == peer) return c.displayName;
    }
    return peer.length > 12 ? '${peer.substring(0, 12)}…' : peer;
  }
}

/// Badge de non-lus (bulle numérotée).
class _UnreadBadge extends StatelessWidget {
  const _UnreadBadge({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        color: theme.colorScheme.primary,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Text(
        '$count',
        style: theme.textTheme.labelSmall
            ?.copyWith(color: theme.colorScheme.onPrimary),
      ),
    );
  }
}

/// Vue d'une conversation : bannière d'invitation éventuelle,
/// historique fusionné messages + pieces jointes, compositeur avec
/// bouton « joindre » et zone de glisser-déposer (ADR-0019 §5).
class ConversationView extends ConsumerStatefulWidget {
  const ConversationView({super.key, required this.tab, this.conv});

  final OpenConvTab tab;
  final MessagingConversation? conv;

  @override
  ConsumerState<ConversationView> createState() => _ConversationViewState();
}

class _ConversationViewState extends ConsumerState<ConversationView> {
  final _controller = TextEditingController();
  bool _dropHover = false;
  bool _busy = false;

  String get _convId => widget.tab.convId;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final history = ref.watch(convHistoryProvider(_convId));
    final attaches = ref.watch(convAttachmentsProvider(_convId));
    final invited = widget.conv?.isInvited == true;

    return DropDetector(
      onHover: (h) => setState(() => _dropHover = h),
      onFiles: _attachFiles,
      child: Column(
        children: [
          if (widget.conv?.isGroup == true) _GroupHeader(conv: widget.conv!),
          if (invited) _InviteBanner(conv: widget.conv!),
          if (_dropHover)
            Container(
              width: double.infinity,
              color: Theme.of(context)
                  .colorScheme
                  .primaryContainer
                  .withValues(alpha: 0.4),
              padding: const EdgeInsets.all(AppSpace.sm),
              child: Text(
                l10n.msgAttachDrop,
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.labelMedium,
              ),
            ),
          Expanded(
            child: history.when(
              loading: () =>
                  const Center(child: CircularProgressIndicator()),
              error: (e, _) => _EmptyPane(
                icon: Icons.error_outline,
                title: l10n.errorMessage('$e'),
              ),
              data: (messages) {
                // Fusion messages + pieces jointes par date
                // d'insertion — `reverse` affiche le plus recent en
                // bas.
                final items = <(int, Object)>[
                  for (final m in messages) (m.createdAt, m),
                  for (final a in attaches.value ?? const <MessagingAttachment>[])
                    (a.createdAt, a),
                ]..sort((a, b) => b.$1.compareTo(a.$1));
                return ListView.builder(
                  reverse: true,
                  padding: const EdgeInsets.all(AppSpace.md),
                  itemCount: items.length,
                  itemBuilder: (ctx, i) {
                    final item = items[i].$2;
                    return switch (item) {
                      MessagingMessage m => _Bubble(
                        message: m,
                        convId: _convId,
                        group: widget.conv?.isGroup == true,
                      ),
                      MessagingAttachment a => _AttachCard(
                        attach: a,
                        convId: _convId,
                      ),
                      _ => const SizedBox.shrink(),
                    };
                  },
                );
              },
            ),
          ),
          const Divider(height: 1),
          _composer(),
        ],
      ),
    );
  }

  Widget _composer() {
    final l10n = context.l10n;
    return Padding(
      padding: const EdgeInsets.all(AppSpace.sm),
      child: Row(
        children: [
          IconButton(
            icon: const Icon(Icons.attach_file, size: 20),
            tooltip: l10n.msgAttachFile,
            onPressed: _busy ? null : _pickAndAttach,
          ),
          Expanded(
            child: TextField(
              controller: _controller,
              decoration: InputDecoration(
                hintText: l10n.msgTypeMessage,
                border: const OutlineInputBorder(),
                isDense: true,
              ),
              onSubmitted: (_) => _send(),
            ),
          ),
          const SizedBox(width: AppSpace.sm),
          IconButton.filled(
            icon: const Icon(Icons.send, size: 18),
            tooltip: l10n.msgSend,
            onPressed: _send,
          ),
        ],
      ),
    );
  }

  Future<void> _send() async {
    final body = _controller.text.trim();
    if (body.isEmpty) return;
    _controller.clear();
    try {
      await ref.read(messagingRepositoryProvider).convSend(_convId, body);
    } catch (e) {
      if (mounted) {
        final msg = e is ApiException && e.statusCode == 404
            ? context.l10n.msgSendFailed
            : context.l10n.errorMessage('$e');
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(msg)));
      }
    }
    ref.invalidate(convHistoryProvider(_convId));
  }

  /// « Joindre » : sélecteur de fichier → offre `attach` (chemin
  /// local en desktop, octets stagés sur web).
  Future<void> _pickAndAttach() async {
    final file = await pickAnyFile();
    if (file == null) return;
    await _attachFiles([file]);
  }

  /// Envoie chaque fichier comme piece jointe : `path` local →
  /// `convAttach(path)` direct ; octets seuls (web) → staging
  /// `uploadBytes` puis `convAttach(uploadId)`.
  Future<void> _attachFiles(List<PickedFile> files) async {
    if (files.isEmpty) return;
    setState(() => _busy = true);
    try {
      for (final f in files) {
        final repo = ref.read(messagingRepositoryProvider);
        if (f.path != null) {
          await repo.convAttach(_convId, path: f.path, name: f.name);
        } else {
          final bytes = await f.readBytes();
          final up = await repo.uploadBytes(f.name, bytes);
          await repo.convAttach(_convId, uploadId: up.uploadId);
        }
      }
    } catch (e) {
      if (mounted) _showError(context, e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
    // L'offre cree une ligne `msg` + une ligne `msg_attachments`.
    ref.invalidate(convHistoryProvider(_convId));
    ref.invalidate(convAttachmentsProvider(_convId));
  }
}

/// En-tête de groupe : nombre de membres → feuille roster, bouton
/// « inviter » (contacts actifs hors roster).
class _GroupHeader extends ConsumerWidget {
  const _GroupHeader({required this.conv});

  final MessagingConversation conv;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final members = ref.watch(groupMembersProvider(conv.convId)).value ?? const [];
    final active = members.where((m) => m.isActive).length;
    return Material(
      color: Theme.of(context).colorScheme.surfaceContainerLow,
      child: ListTile(
        dense: true,
        leading: const Icon(Icons.groups_outlined, size: 20),
        title: Text(
          conv.displayName,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: Text(l10n.msgGroupMembers(active)),
        trailing: Wrap(
          spacing: AppSpace.xs,
          children: [
            IconButton(
              icon: const Icon(Icons.person_add_alt_1_outlined, size: 18),
              tooltip: l10n.msgGroupInvite,
              onPressed: () => _inviteDialog(context, ref, members),
            ),
            IconButton(
              icon: const Icon(Icons.exit_to_app, size: 18),
              tooltip: l10n.msgGroupLeave,
              onPressed: () async {
                try {
                  await ref
                      .read(messagingRepositoryProvider)
                      .groupLeave(conv.convId);
                  ref.invalidate(messagingConversationsProvider);
                } catch (e) {
                  if (context.mounted) _showError(context, e);
                }
              },
            ),
          ],
        ),
        onTap: () => _rosterSheet(context, ref, conv, members),
      ),
    );
  }

  /// Feuille roster : membres, état, « invité par ».
  void _rosterSheet(
    BuildContext context,
    WidgetRef ref,
    MessagingConversation conv,
    List<MessagingMember> members,
  ) {
    final l10n = context.l10n;
    showModalBottomSheet<void>(
      context: context,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              title: Text(
                l10n.msgGroupRoster,
                style: Theme.of(ctx).textTheme.titleMedium,
              ),
            ),
            const Divider(height: 1),
            for (final m in members)
              ListTile(
                dense: true,
                leading: Icon(
                  m.isActive
                      ? Icons.person_outline
                      : Icons.person_off_outlined,
                  size: 18,
                ),
                title: Text(
                  m.displayName,
                  style: m.alias.isEmpty
                      ? const TextStyle(fontFamily: AppFontFamilies.mono, fontSize: 12)
                      : null,
                ),
                subtitle: Text(
                  m.addedBy.isEmpty
                      ? m.state
                      : '${m.state} · ${l10n.msgGroupInvitedBy(_short(m.addedBy))}',
                ),
              ),
          ],
        ),
      ),
    );
  }

  /// Dialogue « inviter » : contacts actifs hors roster.
  Future<void> _inviteDialog(
    BuildContext context,
    WidgetRef ref,
    List<MessagingMember> members,
  ) async {
    final l10n = context.l10n;
    final contacts = ref.read(messagingContactsProvider).value ?? const [];
    final inRoster = members.map((m) => m.memberPk.toLowerCase()).toSet();
    final candidates = [
      for (final c in contacts)
        if (!inRoster.contains(c.publicKey.toLowerCase())) c,
    ];
    final chosen = await showDialog<String>(
      context: context,
      builder: (ctx) => SimpleDialog(
        title: Text(l10n.msgGroupInvite),
        children: [
          for (final c in candidates)
            SimpleDialogOption(
              onPressed: () => Navigator.pop(ctx, c.publicKey),
              child: Text(c.displayName),
            ),
        ],
      ),
    );
    if (chosen == null || !context.mounted) return;
    try {
      await ref
          .read(messagingRepositoryProvider)
          .groupInvite(conv.convId, chosen);
      ref.invalidate(groupMembersProvider(conv.convId));
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }

  static String _short(String pk) =>
      pk.length > 12 ? '${pk.substring(0, 12)}…' : pk;
}

/// Bannière « invitation au groupe » — accepter (`group_accept`)
/// ou décliner (`group_decline`) ; tant que l'invitation est
/// pendante la conv reste `invited`.
class _InviteBanner extends ConsumerWidget {
  const _InviteBanner({required this.conv});

  final MessagingConversation conv;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    return Material(
      color: theme.colorScheme.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: AppSpace.md,
          vertical: AppSpace.sm,
        ),
        child: Row(
          children: [
            Expanded(
              child: Text(
                l10n.msgGroupInviteBanner,
                style: theme.textTheme.bodySmall,
              ),
            ),
            TextButton(
              onPressed: () => _answer(ref, accept: true),
              child: Text(l10n.msgAccept),
            ),
            TextButton(
              onPressed: () => _answer(ref, accept: false),
              child: Text(l10n.msgDecline),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _answer(WidgetRef ref, {required bool accept}) async {
    try {
      final repo = ref.read(messagingRepositoryProvider);
      if (accept) {
        await repo.groupAccept(conv.convId);
      } else {
        await repo.groupDecline(conv.convId);
        ref.read(openConversationsProvider.notifier).close(conv.convId);
      }
      ref.invalidate(messagingConversationsProvider);
      ref.invalidate(groupMembersProvider(conv.convId));
    } catch (e) {
      // Pas de contexte monté fiable ici (StatelessWidget) : le
      // SSE `messaging_conv` re-synchronisera de toute façon.
    }
  }
}

/// Bulle de message — alignement + auteur en groupe + pastille de
/// statut (outgoing), suppression au clic long.
class _Bubble extends ConsumerWidget {
  const _Bubble({
    required this.message,
    required this.convId,
    required this.group,
  });

  final MessagingMessage message;
  final String convId;
  final bool group;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final statusLabel = switch (message.status) {
      'sent' => l10n.msgStatusSent,
      'acked' => l10n.msgStatusAcked,
      'failed' => l10n.msgStatusFailed,
      _ => l10n.msgStatusReceived,
    };
    final statusIcon = switch (message.status) {
      'sent' => Icons.check,
      'acked' => Icons.done_all,
      'failed' => Icons.error_outline,
      _ => null,
    };
    // En groupe, l'auteur d'un message entrant est affiché au-dessus
    // de la bulle (`author_pk` — l'alias vient du roster/contacts).
    final author = group && !message.isOutgoing
        ? _authorLabel(ref, message.authorPk)
        : null;
    return Align(
      alignment: message.isOutgoing
          ? Alignment.centerRight
          : Alignment.centerLeft,
      child: GestureDetector(
        onLongPress: () => _delete(context, ref),
        child: Container(
          constraints: const BoxConstraints(maxWidth: 480),
          margin: const EdgeInsets.symmetric(vertical: AppSpace.xs / 2),
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpace.md,
            vertical: AppSpace.sm,
          ),
          decoration: BoxDecoration(
            color: message.isOutgoing
                ? theme.colorScheme.primaryContainer
                : theme.colorScheme.surfaceContainerHighest,
            borderRadius: BorderRadius.circular(AppRadius.medium),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              if (author != null)
                Align(
                  alignment: Alignment.centerLeft,
                  child: Text(
                    author,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.primary,
                      fontFamily: AppFontFamilies.mono,
                    ),
                  ),
                ),
              Text(message.body, style: theme.textTheme.bodyMedium),
              if (statusIcon != null)
                Padding(
                  padding: const EdgeInsets.only(top: AppSpace.xs / 2),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(
                        statusIcon,
                        size: 12,
                        color: message.isFailed
                            ? theme.colorScheme.error
                            : theme.colorScheme.onSurfaceVariant,
                      ),
                      const SizedBox(width: AppSpace.xs),
                      Text(
                        statusLabel,
                        style: theme.textTheme.labelSmall?.copyWith(
                          color: message.isFailed
                              ? theme.colorScheme.error
                              : theme.colorScheme.onSurfaceVariant,
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  String? _authorLabel(WidgetRef ref, String? pk) {
    if (pk == null || pk.isEmpty) return null;
    final members = ref.watch(groupMembersProvider(convId)).value ?? const [];
    for (final m in members) {
      if (m.memberPk == pk) return m.displayName;
    }
    final contacts = ref.watch(messagingContactsProvider).value ?? const [];
    for (final c in contacts) {
      if (c.publicKey == pk) return c.displayName;
    }
    return pk.length > 12 ? '${pk.substring(0, 12)}…' : pk;
  }

  Future<void> _delete(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.msgDeleteMessage),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.msgDeleteMessage),
          ),
        ],
      ),
    );
    if (ok != true || !context.mounted) return;
    try {
      await ref.read(messagingRepositoryProvider).deleteMessage(message.id);
      ref.invalidate(convHistoryProvider(convId));
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
  }
}

/// Carte de piece jointe : nom, taille, état ; entrante `offered`
/// → boutons Télécharger/Refuser ; sortante → état de seeding et
/// fan-out.
class _AttachCard extends ConsumerWidget {
  const _AttachCard({required this.attach, required this.convId});

  final MessagingAttachment attach;
  final String convId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final stateLabel = switch (attach.state) {
      'offered' => l10n.msgAttachStateOffered,
      'seeding' => l10n.msgAttachStateSeeding,
      'accepted' => l10n.msgAttachStateAccepted,
      'downloading' => l10n.msgAttachStateDownloading,
      'done' => l10n.msgAttachStateDone,
      'declined' => l10n.msgAttachStateDeclined,
      'expired' => l10n.msgAttachStateExpired,
      _ => attach.state,
    };
    return Align(
      alignment:
          attach.isIncoming ? Alignment.centerLeft : Alignment.centerRight,
      child: Card(
        margin: const EdgeInsets.symmetric(vertical: AppSpace.xs / 2),
        child: Padding(
          padding: const EdgeInsets.all(AppSpace.sm),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(Icons.insert_drive_file_outlined, size: 20),
                  const SizedBox(width: AppSpace.sm),
                  Flexible(
                    child: Text(
                      attach.name,
                      style: theme.textTheme.bodyMedium,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
              ),
              Text(
                '${_sizeLabel(attach.size)} · $stateLabel',
                style: theme.textTheme.labelSmall
                    ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
              ),
              if (attach.isActionable)
                Padding(
                  padding: const EdgeInsets.only(top: AppSpace.xs),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      FilledButton.tonalIcon(
                        icon: const Icon(Icons.download, size: 16),
                        label: Text(l10n.msgAttachDownload),
                        onPressed: () => _acceptDialog(context, ref),
                      ),
                      const SizedBox(width: AppSpace.sm),
                      TextButton(
                        onPressed: () => _decline(context, ref),
                        child: Text(l10n.msgDecline),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  /// Dialogue « destination » : public `@public/messaging` (defaut
  /// `attach_area`) ou prive `@private` — `409 identity_locked`
  /// remonte en snackbar.
  Future<void> _acceptDialog(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    var area = 'public';
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setState) => AlertDialog(
          title: Text(l10n.msgAttachDestination),
          content: RadioGroup<String>(
            groupValue: area,
            onChanged: (v) => setState(() => area = v ?? 'public'),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                RadioListTile<String>(
                  dense: true,
                  title: Text(l10n.msgAttachPublic),
                  value: 'public',
                ),
                RadioListTile<String>(
                  dense: true,
                  title: Text(l10n.msgAttachPrivate),
                  value: 'private',
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
              child: Text(l10n.msgAttachDownload),
            ),
          ],
        ),
      ),
    );
    if (ok != true || !context.mounted) return;
    try {
      await ref
          .read(messagingRepositoryProvider)
          .attachAccept(attach.attachId, area: area);
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
    ref.invalidate(convAttachmentsProvider(convId));
  }

  Future<void> _decline(BuildContext context, WidgetRef ref) async {
    try {
      await ref
          .read(messagingRepositoryProvider)
          .attachDecline(attach.attachId);
    } catch (e) {
      if (context.mounted) _showError(context, e);
    }
    ref.invalidate(convAttachmentsProvider(convId));
  }

  static String _sizeLabel(int bytes) {
    if (bytes >= 1 << 30) return '${(bytes / (1 << 30)).toStringAsFixed(1)} Gio';
    if (bytes >= 1 << 20) return '${(bytes / (1 << 20)).toStringAsFixed(1)} Mio';
    if (bytes >= 1 << 10) return '${(bytes / (1 << 10)).toStringAsFixed(1)} Kio';
    return '$bytes o';
  }
}

/// Panneau générique icône + titre (état vide / erreur).
class _EmptyPane extends StatelessWidget {
  const _EmptyPane({required this.icon, required this.title});

  final IconData icon;
  final String title;

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
          ],
        ),
      ),
    );
  }
}

void _showError(BuildContext context, Object e) {
  final msg = e is ApiException ? e.message : context.l10n.errorMessage('$e');
  ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(msg)));
}
