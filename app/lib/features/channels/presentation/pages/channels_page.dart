// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/design/design_tokens.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';
import '../../../downloads/presentation/widgets/add_download_dialog.dart';
import '../../../search/domain/torrent_result.dart';
import '../../domain/channel.dart';
import '../providers/channels_providers.dart';

/// Page « Canaux » — abonnements aux canaux curés signés Ed25519
/// (ADR-0025) : liste, abonnement par clé, contenu synchronisé avec
/// santé jointe, désabonnement.
class ChannelsPage extends ConsumerWidget {
  const ChannelsPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final channels = ref.watch(channelsProvider);
    final l10n = context.l10n;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpace.md,
            vertical: AppSpace.sm,
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  l10n.channelsSubscribed,
                  style: Theme.of(context).textTheme.titleSmall,
                ),
              ),
              IconButton(
                tooltip: l10n.refresh,
                onPressed: ref.read(channelsProvider.notifier).refresh,
                icon: const Icon(Icons.refresh, size: 20),
              ),
              FilledButton.tonalIcon(
                icon: const Icon(Icons.add, size: 18),
                label: Text(l10n.channelsFollow),
                onPressed: () => _SubscribeDialog.show(context),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.only(
            left: AppSpace.md,
            right: AppSpace.md,
            bottom: AppSpace.sm,
          ),
          child: Text(
            l10n.channelsInfo,
            style: Theme.of(context).textTheme.bodySmall?.copyWith(
              color: Theme.of(context).colorScheme.onSurfaceVariant,
            ),
          ),
        ),
        Expanded(
          child: channels.when(
            loading: () => const Center(child: CircularProgressIndicator()),
            error: (e, _) => ErrorState(
              error: e,
              onRetry: () => ref.invalidate(channelsProvider),
            ),
            data: (list) => list.isEmpty
                ? EmptyState(
                    icon: Icons.collections_bookmark_outlined,
                    title: l10n.channelsEmptyTitle,
                    message: l10n.channelsEmptyMessage,
                  )
                : ListView.builder(
                    padding: const EdgeInsets.symmetric(
                      horizontal: AppSpace.sm,
                    ),
                    itemCount: list.length,
                    itemBuilder: (context, i) => _ChannelCard(channel: list[i]),
                  ),
          ),
        ),
      ],
    );
  }
}

/// Canal suivi : en-tête (titre curé + badge signé + compteur) et
/// contenu synchronisé dépliable.
class _ChannelCard extends ConsumerWidget {
  const _ChannelCard({required this.channel});

  final Channel channel;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    return Card(
      margin: const EdgeInsets.only(bottom: AppSpace.sm),
      clipBehavior: Clip.antiAlias,
      child: ExpansionTile(
        leading: const Icon(Icons.collections_bookmark_outlined),
        title: Row(
          children: [
            Expanded(
              child: Text(
                channel.name.isEmpty ? l10n.channelsUnnamed : channel.name,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            // Badge « curé » : le contenu n'entre que s'il est signé
            // par la clé `public_key` du canal (anti-poisoning).
            Tooltip(
              message: l10n.channelsCuratedTooltip,
              child: Padding(
                padding: const EdgeInsets.only(right: AppSpace.xs),
                child: Icon(
                  Icons.verified_outlined,
                  size: 16,
                  color: theme.colorScheme.primary,
                ),
              ),
            ),
          ],
        ),
        subtitle: Text(
          [
            l10n.channelsEntries(channel.numEntries),
            '${channel.publicKey.substring(0, 12.clamp(0, channel.publicKey.length))}…',
          ].join(' · '),
          style: theme.textTheme.bodySmall,
        ),
        trailing: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            IconButton(
              tooltip: l10n.channelsUnsubscribe,
              icon: const Icon(Icons.bookmark_remove_outlined, size: 20),
              onPressed: () => _confirmUnsubscribe(context, ref, channel),
            ),
            const Icon(Icons.expand_more, size: 20),
          ],
        ),
        children: [
          const Divider(height: 1),
          _ChannelContents(channel: channel),
        ],
      ),
    );
  }

  Future<void> _confirmUnsubscribe(
    BuildContext context,
    WidgetRef ref,
    Channel channel,
  ) async {
    final l10n = context.l10n;
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(l10n.channelsUnsubscribeTitle),
        content: Text(
          l10n.channelsUnsubscribeBody(
            channel.name.isEmpty ? l10n.channelsUnnamed : channel.name,
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(l10n.channelsUnsubscribe),
          ),
        ],
      ),
    );
    if (ok != true || !context.mounted) return;
    try {
      await ref.read(channelsProvider.notifier).unsubscribe(channel);
    } catch (e) {
      if (!context.mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(l10n.channelsError('$e'))));
    }
  }
}

/// Contenu du canal — charge le provider à l'expansion uniquement
/// (les tuiles restent dans l'arbre tant que la carte est ouverte).
class _ChannelContents extends ConsumerWidget {
  const _ChannelContents({required this.channel});

  final Channel channel;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final key = (publicKey: channel.publicKey, id: channel.id);
    final contents = ref.watch(channelContentsProvider(key));
    return contents.when(
      loading: () => const Padding(
        padding: EdgeInsets.all(AppSpace.md),
        child: Center(child: CircularProgressIndicator()),
      ),
      error: (e, _) => Padding(
        padding: const EdgeInsets.all(AppSpace.md),
        child: ErrorState(
          error: e,
          onRetry: () => ref.invalidate(channelContentsProvider(key)),
        ),
      ),
      data: (results) => results.isEmpty
          ? Padding(
              padding: const EdgeInsets.all(AppSpace.md),
              child: Text(
                l10n.channelsSyncPending,
                style: Theme.of(context).textTheme.bodySmall,
              ),
            )
          : Column(
              children: [for (final r in results) _ChannelEntryTile(result: r)],
            ),
    );
  }
}

class _ChannelEntryTile extends StatelessWidget {
  const _ChannelEntryTile({required this.result});

  final TorrentResult result;

  @override
  Widget build(BuildContext context) {
    final r = result;
    return ListTile(
      dense: true,
      leading: const Icon(Icons.cloud_outlined, size: 20),
      title: Text(
        r.name.isEmpty ? r.infohash : r.name,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Text(
        [
          context.fmtBytes(r.size),
          if (r.seeders != null)
            context.l10n.resultSeedsLeechers(r.seeders!, r.leechers ?? 0),
        ].join(' · '),
        style: Theme.of(context).textTheme.bodySmall,
      ),
      trailing: FilledButton.tonalIcon(
        onPressed: r.infohash.isEmpty
            ? null
            : () => AddDownloadDialog.show(context, initialUri: r.magnet),
        icon: const Icon(Icons.download, size: 18),
        label: Text(context.l10n.add),
      ),
      onTap: r.infohash.isEmpty
          ? null
          : () => AddDownloadDialog.show(context, initialUri: r.magnet),
    );
  }
}

/// Dialogue d'abonnement : `(public_key` hex 64 o + `origin_id)`.
class _SubscribeDialog extends ConsumerStatefulWidget {
  const _SubscribeDialog();

  static Future<void> show(BuildContext context) => showDialog<void>(
    context: context,
    builder: (_) => const _SubscribeDialog(),
  );

  @override
  ConsumerState<_SubscribeDialog> createState() => _SubscribeDialogState();
}

class _SubscribeDialogState extends ConsumerState<_SubscribeDialog> {
  final _pkController = TextEditingController();
  final _idController = TextEditingController(text: '0');
  bool _busy = false;

  @override
  void dispose() {
    _pkController.dispose();
    _idController.dispose();
    super.dispose();
  }

  bool get _pkValid =>
      RegExp(r'^[0-9a-fA-F]{128}$').hasMatch(_pkController.text.trim());

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return AlertDialog(
      title: Text(l10n.channelsFollowTitle),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            controller: _pkController,
            decoration: InputDecoration(
              labelText: l10n.channelsPublicKeyLabel,
              hintText: l10n.channelsPublicKeyHint,
            ),
            onChanged: (_) => setState(() {}),
          ),
          const SizedBox(height: AppSpace.sm),
          TextField(
            controller: _idController,
            decoration: InputDecoration(labelText: l10n.channelsIdLabel),
            keyboardType: TextInputType.number,
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: _busy || !_pkValid ? null : _submit,
          child: Text(l10n.channelsFollow),
        ),
      ],
    );
  }

  Future<void> _submit() async {
    setState(() => _busy = true);
    try {
      await ref
          .read(channelsProvider.notifier)
          .subscribe(
            _pkController.text.trim(),
            int.tryParse(_idController.text.trim()) ?? 0,
          );
      if (mounted) Navigator.of(context).pop();
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(context.l10n.channelsError('$e'))));
    }
  }
}
