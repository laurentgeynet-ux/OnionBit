// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/platform/pick_directory.dart';
import '../../../../core/platform/pick_file.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../settings/presentation/providers/settings_providers.dart';
import '../../domain/torrent_preview.dart';
import '../providers/downloads_providers.dart';

/// Dialogue « Ajouter » — magnet/URI, fichier `.torrent` (binaire,
/// fonctionne aussi sur web), destination optionnelle et choix
/// d'anonymat binaire+sauts (règle backend : `anon_hops>0` exige
/// `safe_seeding`, envoyé automatiquement).
class AddDownloadDialog extends ConsumerStatefulWidget {
  const AddDownloadDialog({super.key, this.initialUri, this.initialFile});

  /// URI pré-remplie (ex. magnet d'un résultat de recherche).
  final String? initialUri;

  /// `.torrent`/`.magnet` pré-sélectionné (association de fichiers /
  /// glisser-déposer / picker). `.magnet` → son URI est lue dans le
  /// champ magnet.
  final PickedFile? initialFile;

  static Future<void> show(
    BuildContext context, {
    String? initialUri,
    PickedFile? initialFile,
  }) => showDialog(
    context: context,
    builder: (_) => AddDownloadDialog(
      initialUri: initialUri,
      initialFile: initialFile,
    ),
  );

  @override
  ConsumerState<AddDownloadDialog> createState() => _AddDownloadDialogState();
}

class _AddDownloadDialogState extends ConsumerState<AddDownloadDialog> {
  late final TextEditingController _uriController = TextEditingController(
    text: widget.initialUri,
  );
  final _destController = TextEditingController();
  String? _fileName;
  Uint8List? _fileBytes;
  TorrentPreview? _preview;

  /// Sauts choisis — initialisés depuis `download_defaults` quand les
  /// réglages arrivent (comme `ask_download_settings` Tribler qui
  /// pré-remplit le dialogue avec les défauts configurés).
  int _hops = 0;
  bool _hopsInitialized = false;
  bool _paused = false;
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    final file = widget.initialFile;
    if (file != null) {
      file
          .readBytes()
          .then((bytes) {
            if (!mounted) return;
            if (file.name.toLowerCase().endsWith('.magnet')) {
              // Fichier .magnet : contient l'URI en clair.
              _uriController.text = utf8.decode(bytes).trim();
            } else {
              setState(() {
                _fileName = file.name;
                _fileBytes = bytes;
              });
              _loadPreview(bytes);
            }
          })
          .catchError((_) {});
    }
  }

  @override
  void dispose() {
    _uriController.dispose();
    _destController.dispose();
    super.dispose();
  }

  bool get _canSubmit =>
      !_busy && (_uriController.text.trim().isNotEmpty || _fileBytes != null);

  /// Torrent privé détecté via l'aperçu metainfo (`private=1`) : le
  /// tracker exige une connexion directe (passkey liée à l'IP) et le
  /// flag interdit DHT/PEX — aucun chemin anonyme n'existe. Le
  /// choix de sauts est alors verrouillé sur « Clair ».
  bool get _privateTorrent => _preview?.isPrivate ?? false;

  Future<void> _pickFile() async {
    final file = await pickTorrentFile();
    if (file == null) return;
    final bytes = await file.readBytes();
    setState(() {
      _fileName = file.name;
      _fileBytes = bytes;
      _preview = null;
    });
    _loadPreview(bytes);
  }

  /// Aperçu des trackers pour l'avertissement HTTPS-only — echec
  /// silencieux : le torrent reste ajoutable sans l'alerte.
  Future<void> _loadPreview(Uint8List bytes) async {
    try {
      final preview = await ref
          .read(downloadsRepositoryProvider)
          .previewTorrentFile(bytes);
      if (mounted && _fileBytes == bytes) {
        setState(() {
          _preview = preview;
          // Torrent privé : l'anonymat est impossible — le choix
          // repasse sur « Clair » immédiatement.
          if (preview.isPrivate) _hops = 0;
        });
      }
    } catch (_) {
      // Metainfo illisible : l'ajout remontera l'erreur proprement.
    }
  }

  Future<void> _submit() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final repo = ref.read(downloadsRepositoryProvider);
    try {
      if (_fileBytes != null) {
        await repo.addTorrentBytes(
          _fileBytes!,
          destination: _destController.text.trim().isEmpty
              ? null
              : _destController.text.trim(),
          anonHops: _hops,
          safeSeeding: _hops > 0,
          paused: _paused,
        );
      } else {
        final uri = _uriController.text.trim();
        // Magnet uniquement : une URL http(s) exposerait l'interet
        // pour ce .torrent en clair (GET du metainfo). Le fichier
        // telecharge separement se choisit via le picker — zero
        // reseau a l'ajout.
        if (!uri.startsWith('magnet:')) {
          setState(() => _error = context.l10n.uriMustBeMagnet);
          return;
        }
        await repo.add(
          uri: uri,
          destination: _destController.text.trim().isEmpty
              ? null
              : _destController.text.trim(),
          anonHops: _hops,
          safeSeeding: _hops > 0,
          paused: _paused,
        );
      }
      if (mounted) {
        Navigator.of(context).pop();
        ref.read(downloadsProvider.notifier).refresh();
      }
    } on ApiException catch (e) {
      setState(() => _error = e.message);
    } catch (e) {
      setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Pré-remplit les sauts depuis `libtorrent/download_defaults`
  /// (`anonymity_enabled` + `number_hops`) une fois les réglages
  /// chargés — l'utilisateur peut toujours les changer ensuite.
  void _initHops(Map<String, dynamic>? settings) {
    if (_hopsInitialized || settings == null) return;
    _hopsInitialized = true;
    final dd = settings['libtorrent'] is Map
        ? (settings['libtorrent'] as Map)['download_defaults']
        : null;
    if (dd is! Map) return;
    final anonymous = dd['anonymity_enabled'] == true;
    final hops = (dd['number_hops'] as num?)?.toInt() ?? 0;
    // Appelé pendant build : affectation directe, pas de setState.
    _hops = anonymous ? hops.clamp(1, 3) : 0;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    _initHops(ref.watch(daemonSettingsProvider).value);
    // Torrent privé : l'anonymat est structurellement impossible —
    // les sauts restent verrouillés sur « Clair » quelle que soit
    // l'init tardive des réglages ou un choix antérieur.
    if (_privateTorrent) _hops = 0;
    return AlertDialog(
      title: Text(l10n.addDownload),
      content: SizedBox(
        width: 480,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: _uriController,
              enabled: _fileBytes == null,
              decoration: InputDecoration(
                labelText: l10n.magnetOrUrl,
                hintText: l10n.magnetHint,
                prefixIcon: const Icon(Icons.link),
              ),
              onChanged: (_) => setState(() {}),
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                OutlinedButton.icon(
                  onPressed: _pickFile,
                  icon: const Icon(Icons.upload_file),
                  label: Text(_fileName ?? l10n.torrentFileBtn),
                ),
                if (_fileBytes != null)
                  IconButton(
                    tooltip: l10n.removeFile,
                    onPressed: () => setState(() {
                      _fileName = null;
                      _fileBytes = null;
                      _preview = null;
                    }),
                    icon: const Icon(Icons.close),
                  ),
              ],
            ),
            if (_privateTorrent) ...[
              const SizedBox(height: AppSpacing.sm),
              Container(
                padding: const EdgeInsets.all(AppSpacing.sm),
                decoration: BoxDecoration(
                  color: theme.colorScheme.errorContainer,
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: theme.colorScheme.error),
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Icon(
                      Icons.gpp_bad_outlined,
                      color: theme.colorScheme.error,
                      size: 22,
                    ),
                    const SizedBox(width: AppSpacing.sm),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            l10n.privateTorrentTitle,
                            style: theme.textTheme.labelLarge?.copyWith(
                              color: theme.colorScheme.onErrorContainer,
                            ),
                          ),
                          const SizedBox(height: 2),
                          Text(
                            l10n.privateTorrentWarn,
                            style: theme.textTheme.bodySmall?.copyWith(
                              color: theme.colorScheme.onErrorContainer,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ],
            const SizedBox(height: AppSpacing.md),
            Text(l10n.rowAnon, style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpacing.xs),
            SegmentedButton<int>(
              segments: [
                ButtonSegment(value: 0, label: Text(l10n.anonDirect)),
                for (final n in const [1, 2, 3])
                  ButtonSegment(
                    value: n,
                    enabled: !_privateTorrent,
                    label: Text(l10n.ctxHops(n)),
                  ),
              ],
              selected: {_hops},
              // Le choix explicite verrouille `_hops` : les réglages
              // (`_initHops`) arrivant en retard ne l'écrasent plus.
              onSelectionChanged: (s) => setState(() {
                _hops = s.first;
                _hopsInitialized = true;
              }),
            ),
            const SizedBox(height: AppSpacing.xs),
            Text(
              _hops == 0 ? l10n.anonDirectWarn : l10n.anonRelaysInfo(_hops),
              style: theme.textTheme.bodySmall?.copyWith(
                // Mode clair : IP exposee — avertissement en rouge,
                // pas une simple note (meme palette que le badge
                // « Clair » de la liste).
                color: _hops == 0
                    ? theme.colorScheme.error
                    : theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: AppSpacing.md),
            TextField(
              controller: _destController,
              decoration: InputDecoration(
                labelText: l10n.destFolderOpt,
                prefixIcon: const Icon(Icons.folder_outlined),
                suffixIcon: IconButton(
                  tooltip: l10n.browse,
                  icon: const Icon(Icons.folder_open),
                  onPressed: () async {
                    final dir = await pickDaemonDirectory(
                      context,
                      initialPath: _destController.text.trim(),
                    );
                    if (dir != null) {
                      setState(() => _destController.text = dir);
                    }
                  },
                ),
              ),
            ),
            CheckboxListTile(
              value: _paused,
              onChanged: (v) => setState(() => _paused = v ?? false),
              title: Text(l10n.addPaused),
              contentPadding: EdgeInsets.zero,
              controlAffinity: ListTileControlAffinity.leading,
              dense: true,
            ),
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
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: _canSubmit ? _submit : null,
          child: _busy
              ? const SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : Text(l10n.add),
        ),
      ],
    );
  }
}
