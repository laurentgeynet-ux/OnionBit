// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/widgets/daemon_directory_picker.dart';
import '../providers/downloads_providers.dart';

/// Dialogue « Créer un torrent » : construit un `.torrent` depuis un
/// fichier/dossier de la machine du daemon
/// (`POST /api/createtorrent`) puis, optionnellement, l'ajoute au
/// partage (`PUT /api/downloads torrent=…`) sur la lane choisie —
/// Clair ou Anon ×1/×2/×3, même sélecteur que le dialogue d'ajout.
class CreateTorrentDialog extends ConsumerStatefulWidget {
  const CreateTorrentDialog({super.key});

  static Future<void> show(BuildContext context) => showDialog<void>(
    context: context,
    builder: (_) => const CreateTorrentDialog(),
  );

  @override
  ConsumerState<CreateTorrentDialog> createState() =>
      _CreateTorrentDialogState();
}

class _CreateTorrentDialogState extends ConsumerState<CreateTorrentDialog> {
  final _srcCtrl = TextEditingController();
  final _nameCtrl = TextEditingController();
  final _trackerCtrl = TextEditingController();
  final _descCtrl = TextEditingController();
  final _exportCtrl = TextEditingController();
  int _hops = 0;
  bool _seed = true;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    for (final c in [
      _srcCtrl,
      _nameCtrl,
      _trackerCtrl,
      _descCtrl,
      _exportCtrl,
    ]) {
      c.dispose();
    }
    super.dispose();
  }

  Future<void> _submit() async {
    final l10n = context.l10n;
    final src = _srcCtrl.text.trim();
    if (src.isEmpty) {
      setState(() => _error = l10n.ctSourceRequired);
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final repo = ref.read(downloadsRepositoryProvider);
      final res = await repo.createTorrent(
        files: [src],
        name: _nameCtrl.text.trim().isEmpty ? null : _nameCtrl.text.trim(),
        description: _descCtrl.text.trim().isEmpty
            ? null
            : _descCtrl.text.trim(),
        tracker: _trackerCtrl.text.trim().isEmpty
            ? null
            : _trackerCtrl.text.trim(),
        exportDir: _exportCtrl.text.trim().isEmpty
            ? null
            : _exportCtrl.text.trim(),
      );
      var seeded = false;
      // `torrent=` attend le chemin du .torrent produit (ligne
      // `path` de la reponse) — il vit sur la machine du daemon.
      if (_seed && res.path.isNotEmpty) {
        await repo.add(torrentPath: res.path, anonHops: _hops);
        seeded = true;
      }
      if (!mounted) return;
      Navigator.of(context).pop();
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            seeded
                ? l10n.ctSeededSuccess(res.path)
                : l10n.ctSuccess(res.path),
          ),
        ),
      );
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = '$e';
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    return AlertDialog(
      title: Text(l10n.createTorrent),
      content: SizedBox(
        width: 480,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              TextField(
                controller: _srcCtrl,
                autofocus: true,
                decoration: InputDecoration(
                  labelText: l10n.ctSource,
                  helperText: l10n.ctSourceHint,
                  suffixIcon: IconButton(
                    tooltip: l10n.ctBrowseDir,
                    icon: const Icon(Icons.folder_open_outlined, size: 20),
                    onPressed: () async {
                      final dir = await DaemonDirectoryPicker.show(
                        context,
                        initialPath: _srcCtrl.text.trim(),
                      );
                      if (dir != null && dir.isNotEmpty) {
                        _srcCtrl.text = dir;
                      }
                    },
                  ),
                ),
              ),
              const SizedBox(height: AppSpacing.sm),
              TextField(
                controller: _nameCtrl,
                decoration: InputDecoration(labelText: l10n.ctName),
              ),
              const SizedBox(height: AppSpacing.sm),
              TextField(
                controller: _trackerCtrl,
                decoration: InputDecoration(
                  labelText: l10n.ctTracker,
                  hintText: 'https://tracker.example/announce',
                ),
              ),
              const SizedBox(height: AppSpacing.sm),
              TextField(
                controller: _descCtrl,
                decoration: InputDecoration(labelText: l10n.ctDesc),
              ),
              const SizedBox(height: AppSpacing.sm),
              TextField(
                controller: _exportCtrl,
                decoration: InputDecoration(
                  labelText: l10n.ctExport,
                  helperText: l10n.ctExportHint,
                  suffixIcon: IconButton(
                    tooltip: l10n.ctBrowseDir,
                    icon: const Icon(Icons.folder_open_outlined, size: 20),
                    onPressed: () async {
                      final dir = await DaemonDirectoryPicker.show(
                        context,
                        initialPath: _exportCtrl.text.trim(),
                      );
                      if (dir != null && dir.isNotEmpty) {
                        _exportCtrl.text = dir;
                      }
                    },
                  ),
                ),
              ),
              const SizedBox(height: AppSpacing.md),
              CheckboxListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                title: Text(
                  l10n.ctSeed,
                  style: theme.textTheme.bodyMedium,
                ),
                value: _seed,
                onChanged: (v) => setState(() => _seed = v ?? false),
              ),
              if (_seed) ...[
                const SizedBox(height: AppSpacing.xs),
                SegmentedButton<int>(
                  segments: [
                    ButtonSegment(value: 0, label: Text(l10n.anonDirect)),
                    for (final n in const [1, 2, 3])
                      ButtonSegment(value: n, label: Text(l10n.ctxHops(n))),
                  ],
                  selected: {_hops},
                  onSelectionChanged: (s) =>
                      setState(() => _hops = s.first),
                ),
                const SizedBox(height: AppSpacing.xs),
                Text(
                  _hops == 0
                      ? l10n.anonDirectWarn
                      : l10n.anonRelaysInfo(_hops),
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: _hops == 0
                        ? theme.colorScheme.error
                        : theme.colorScheme.outline,
                  ),
                ),
              ],
              if (_error != null) ...[
                const SizedBox(height: AppSpacing.sm),
                Text(
                  _error!,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.error,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: _busy ? null : _submit,
          child: _busy
              ? const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : Text(l10n.ctCreate),
        ),
      ],
    );
  }
}
