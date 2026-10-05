// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../domain/messaging_contact.dart';
import '../../domain/messaging_message.dart';
import '../providers/messaging_providers.dart';

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
        return const Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SizedBox(width: 300, child: _ContactsPane()),
            VerticalDivider(width: 1),
            Expanded(child: _ConversationPane()),
          ],
        );
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
        padding: const EdgeInsets.all(AppSpacing.lg),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 48, color: theme.colorScheme.onSurfaceVariant),
            const SizedBox(height: AppSpacing.md),
            Text(title, style: theme.textTheme.titleMedium),
            if (body != null) ...[
              const SizedBox(height: AppSpacing.sm),
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
    final selected = ref.watch(selectedContactProvider);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Bandeau adresse locale — copie au clic.
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
                  ?.copyWith(fontFamily: 'monospace'),
            ),
            trailing: IconButton(
              icon: const Icon(Icons.copy, size: 18),
              tooltip: l10n.msgYourAddress,
              onPressed: () =>
                  Clipboard.setData(ClipboardData(text: stats.publicKey)),
            ),
          ),
        const Divider(height: 1),
        Expanded(
          child: ListView(
            children: [
              // Demandes de consentement en attente — actions
              // accepter / refuser / bloquer.
              if (pending.isNotEmpty) ...[
                Padding(
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.md,
                    AppSpacing.md,
                    AppSpacing.md,
                    AppSpacing.xs,
                  ),
                  child: Text(
                    l10n.msgPendingTitle,
                    style: Theme.of(context).textTheme.labelLarge,
                  ),
                ),
                for (final c in pending) _PendingTile(contact: c),
                const Divider(height: AppSpacing.lg),
              ],
              if (contacts.isEmpty && pending.isEmpty)
                Padding(
                  padding: const EdgeInsets.all(AppSpacing.lg),
                  child: Text(
                    l10n.msgNoContacts,
                    style: Theme.of(context).textTheme.bodyMedium,
                    textAlign: TextAlign.center,
                  ),
                ),
              for (final c in contacts)
                _ContactTile(
                  contact: c,
                  selected: c.publicKey == selected,
                  onTap: () => ref
                      .read(selectedContactProvider.notifier)
                      .set(c.publicKey),
                ),
            ],
          ),
        ),
        const Divider(height: 1),
        // « Ajouter un contact » — résolution + liaison e2e.
        Padding(
          padding: const EdgeInsets.all(AppSpacing.sm),
          child: FilledButton.tonalIcon(
            icon: const Icon(Icons.person_add_outlined, size: 18),
            label: Text(l10n.msgAddContact),
            onPressed: () => _addContact(context, ref),
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
          style: const TextStyle(fontFamily: 'monospace'),
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
      ref.read(selectedContactProvider.notifier).set(pk);
      ref.invalidate(messagingContactsProvider);
      ref.invalidate(messagingHistoryProvider(pk));
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
      title: Text(
        contact.shortKey,
        style: const TextStyle(fontFamily: 'monospace'),
      ),
      subtitle: contact.pendingSinceSecs != null
          ? Text('${contact.pendingSinceSecs} s')
          : null,
      trailing: Wrap(
        spacing: AppSpacing.xs,
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

/// Tuile contact — état, circuit lié, menu blocage/rétention/
/// suppression.
class _ContactTile extends ConsumerWidget {
  const _ContactTile({
    required this.contact,
    required this.selected,
    required this.onTap,
  });

  final MessagingContact contact;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    final repo = ref.watch(messagingRepositoryProvider);
    final stateLabel = switch (contact.state) {
      MessagingContactState.pending => l10n.msgStatePending,
      MessagingContactState.blocked => l10n.msgStateBlocked,
      _ => null,
    };
    return ListTile(
      dense: true,
      selected: selected,
      leading: Icon(
        contact.circuitId != null ? Icons.circle : Icons.circle_outlined,
        size: 10,
        color: contact.circuitId != null
            ? theme.colorScheme.primary
            : theme.colorScheme.outline,
      ),
      title: Text(
        contact.shortKey,
        style: const TextStyle(fontFamily: 'monospace'),
      ),
      subtitle: stateLabel != null ? Text(stateLabel) : null,
      trailing: PopupMenuButton<String>(
        iconSize: 18,
        itemBuilder: (ctx) => [
          if (contact.state == MessagingContactState.blocked)
            PopupMenuItem(value: 'unblock', child: Text(l10n.msgUnblock))
          else
            PopupMenuItem(value: 'block', child: Text(l10n.msgBlock)),
          PopupMenuItem(
            value: 'retention',
            child: Text(l10n.msgRetentionTitle),
          ),
          PopupMenuItem(value: 'delete', child: Text(l10n.msgDeleteContact)),
        ],
        onSelected: (v) => switch (v) {
          'block' => _act(context, ref, () => repo.block(contact.publicKey)),
          'unblock' => _act(
            context,
            ref,
            () => repo.unblock(contact.publicKey),
          ),
          'retention' => _retentionDialog(context, ref),
          _ => _act(context, ref, () => repo.remove(contact.publicKey)),
        },
      ),
      onTap: onTap,
    );
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

/// Panneau droit : historique + compositeur.
class _ConversationPane extends ConsumerStatefulWidget {
  const _ConversationPane();

  @override
  ConsumerState<_ConversationPane> createState() => _ConversationPaneState();
}

class _ConversationPaneState extends ConsumerState<_ConversationPane> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final pk = ref.watch(selectedContactProvider);
    if (pk == null) {
      return _MessagePane(
        icon: Icons.chat_bubble_outline,
        title: l10n.msgSelectContact,
      );
    }
    final history = ref.watch(messagingHistoryProvider(pk));
    return Column(
      children: [
        Expanded(
          child: history.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => _MessagePane(
              icon: Icons.error_outline,
              title: l10n.errorMessage('$e'),
            ),
            data: (messages) {
              // `history` rend le plus récent d'abord — la liste
              // `reverse` affiche le plus récent en bas.
              return ListView.builder(
                reverse: true,
                padding: const EdgeInsets.all(AppSpacing.md),
                itemCount: messages.length,
                itemBuilder: (ctx, i) => _Bubble(message: messages[i]),
              );
            },
          ),
        ),
        const Divider(height: 1),
        // Compositeur — `send` propage le 404 « hors ligne » en
        // snackbar (le daemon enregistre déjà `failed`).
        Padding(
          padding: const EdgeInsets.all(AppSpacing.sm),
          child: Row(
            children: [
              Expanded(
                child: TextField(
                  controller: _controller,
                  decoration: InputDecoration(
                    hintText: l10n.msgTypeMessage,
                    border: const OutlineInputBorder(),
                    isDense: true,
                  ),
                  onSubmitted: (_) => _send(pk),
                ),
              ),
              const SizedBox(width: AppSpacing.sm),
              IconButton.filled(
                icon: const Icon(Icons.send, size: 18),
                tooltip: l10n.msgSend,
                onPressed: () => _send(pk),
              ),
            ],
          ),
        ),
      ],
    );
  }

  Future<void> _send(String pk) async {
    final body = _controller.text.trim();
    if (body.isEmpty) return;
    _controller.clear();
    try {
      await ref.read(messagingRepositoryProvider).send(pk, body);
    } catch (e) {
      if (mounted) {
        final msg = e is ApiException && e.statusCode == 404
            ? context.l10n.msgSendFailed
            : context.l10n.errorMessage('$e');
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(msg)));
      }
    }
    // Toujours rafraîchir : un échec est enregistré `failed` côté
    // daemon (la bulle doit apparaître en erreur).
    ref.invalidate(messagingHistoryProvider(pk));
  }
}

/// Bulle de message — alignement + pastille de statut (outgoing)
/// et suppression au clic long.
class _Bubble extends ConsumerWidget {
  const _Bubble({required this.message});

  final MessagingMessage message;

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
    return Align(
      alignment: message.isOutgoing
          ? Alignment.centerRight
          : Alignment.centerLeft,
      child: GestureDetector(
        onLongPress: () => _delete(context, ref),
        child: Container(
          constraints: const BoxConstraints(maxWidth: 480),
          margin: const EdgeInsets.symmetric(vertical: AppSpacing.xs / 2),
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.md,
            vertical: AppSpacing.sm,
          ),
          decoration: BoxDecoration(
            color: message.isOutgoing
                ? theme.colorScheme.primaryContainer
                : theme.colorScheme.surfaceContainerHighest,
            borderRadius: BorderRadius.circular(AppRadii.medium),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Text(message.body, style: theme.textTheme.bodyMedium),
              if (statusIcon != null)
                Padding(
                  padding: const EdgeInsets.only(top: AppSpacing.xs / 2),
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
                      const SizedBox(width: AppSpacing.xs),
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
      final pk = ref.read(selectedContactProvider);
      if (pk != null) ref.invalidate(messagingHistoryProvider(pk));
    } catch (e) {
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
    final pk = ref.read(selectedContactProvider);
    if (pk != null) ref.invalidate(messagingHistoryProvider(pk));
  } catch (e) {
    if (context.mounted) _showError(context, e);
  }
}

void _showError(BuildContext context, Object e) {
  final msg = e is ApiException ? e.message : context.l10n.errorMessage('$e');
  ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(msg)));
}
