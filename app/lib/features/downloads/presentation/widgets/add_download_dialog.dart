// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../settings/presentation/providers/settings_providers.dart';
import '../../domain/torrent_preview.dart';
import '../providers/downloads_providers.dart';

/// Dialogue « Ajouter » — magnet/URI, fichier `.torrent` (binaire,
/// fonctionne aussi sur web), destination optionnelle et choix
/// d'anonymat binaire+sauts (règle backend : `anon_hops>0` exige
/// `safe_seeding`, envoyé automatiquement).
class AddDownloadDialog extends ConsumerStatefulWidget {
  const AddDownloadDialog({super.key, this.initialUri, this.initialFilePath});

  /// URI pré-remplie (ex. magnet d'un résultat de recherche).
  final String? initialUri;

  /// Chemin d'un `.torrent` pré-sélectionné (association de
  /// fichiers / glisser-déposer). `.magnet` → son URI est lue
  /// dans le champ magnet.
  final String? initialFilePath;

  static Future<void> show(
    BuildContext context, {
    String? initialUri,
    String? initialFilePath,
  }) =>
      showDialog(
        context: context,
        builder: (_) => AddDownloadDialog(
          initialUri: initialUri,
          initialFilePath: initialFilePath,
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
  XFile? _file;
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
    final path = widget.initialFilePath;
    if (path != null) {
      if (path.toLowerCase().endsWith('.magnet')) {
        // Fichier .magnet : contient l'URI en clair.
        XFile(path).readAsString().then((uri) {
          if (mounted) _uriController.text = uri.trim();
        }).catchError((_) {});
      } else {
        _file = XFile(path);
        _loadPreview(_file!);
      }
    }
  }

  @override
  void dispose() {
    _uriController.dispose();
    _destController.dispose();
    super.dispose();
  }

  bool get _canSubmit =>
      !_busy && (_uriController.text.trim().isNotEmpty || _file != null);

  /// Trackers connus avant ajout : metainfo du `.torrent` choisi via
  /// `/api/torrentinfo/file`, ou parametres `tr=` d'un magnet saisi.
  List<String> get _knownTrackers {
    if (_preview != null) return _preview!.trackers;
    final uri = Uri.tryParse(_uriController.text.trim());
    if (uri == null || uri.scheme != 'magnet') return const [];
    return uri.queryParametersAll['tr'] ?? const [];
  }

  /// Tous les trackers connus sont HTTPS → injoignables via les
  /// sorties anonymes (relai HTTP clair one-shot uniquement).
  bool get _httpsOnlyTrackers =>
      _knownTrackers.isNotEmpty &&
      _knownTrackers.every((t) => t.toLowerCase().startsWith('https://'));

  Future<void> _pickFile() async {
    final file = await openFile(
      acceptedTypeGroups: [
        const XTypeGroup(label: 'torrent', extensions: ['torrent']),
      ],
    );
    if (file == null) return;
    setState(() {
      _file = file;
      _preview = null;
    });
    _loadPreview(file);
  }

  /// Aperçu des trackers pour l'avertissement HTTPS-only — echec
  /// silencieux : le torrent reste ajoutable sans l'alerte.
  Future<void> _loadPreview(XFile file) async {
    try {
      final preview = await ref
          .read(downloadsRepositoryProvider)
          .previewTorrentFile(await file.readAsBytes());
      if (mounted && _file == file) setState(() => _preview = preview);
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
      if (_file != null) {
        await repo.addTorrentBytes(
          await _file!.readAsBytes(),
          destination: _destController.text.trim().isEmpty
              ? null
              : _destController.text.trim(),
          anonHops: _hops,
          safeSeeding: _hops > 0,
          paused: _paused,
        );
      } else {
        await repo.add(
          uri: _uriController.text.trim(),
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
    _initHops(ref.watch(daemonSettingsProvider).value);
    return AlertDialog(
      title: const Text('Ajouter un téléchargement'),
      content: SizedBox(
        width: 480,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: _uriController,
              enabled: _file == null,
              decoration: const InputDecoration(
                labelText: 'Magnet ou URL',
                hintText: 'magnet:?xt=… ou https://…',
                prefixIcon: Icon(Icons.link),
              ),
              onChanged: (_) => setState(() {}),
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                OutlinedButton.icon(
                  onPressed: _pickFile,
                  icon: const Icon(Icons.upload_file),
                  label: Text(_file?.name ?? 'Fichier .torrent…'),
                ),
                if (_file != null)
                  IconButton(
                    tooltip: 'Retirer le fichier',
                    onPressed: () => setState(() {
                      _file = null;
                      _preview = null;
                    }),
                    icon: const Icon(Icons.close),
                  ),
              ],
            ),
            const SizedBox(height: AppSpacing.md),
            Text('Anonymat', style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpacing.xs),
            SegmentedButton<int>(
              segments: const [
                ButtonSegment(value: 0, label: Text('Direct')),
                ButtonSegment(value: 1, label: Text('1 saut')),
                ButtonSegment(value: 2, label: Text('2 sauts')),
                ButtonSegment(value: 3, label: Text('3 sauts')),
              ],
              selected: {_hops},
              onSelectionChanged: (s) => setState(() => _hops = s.first),
            ),
            const SizedBox(height: AppSpacing.xs),
            Text(
              _hops == 0
                  ? 'Votre IP est visible par les pairs.'
                  : '$_hops relais entre vous et l\'essaim. Si aucun '
                        'circuit n\'est prêt, le téléchargement attendra '
                        '(jamais de repli en direct). Le seeding sera aussi '
                        'anonymisé (safe seeding).',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            if (_hops > 0 && _httpsOnlyTrackers) ...[
              const SizedBox(height: AppSpacing.xs),
              Text(
                'Tous les trackers de ce torrent sont en HTTPS : ils '
                'sont injoignables via les sorties anonymes (relai '
                'HTTP en clair uniquement). En mode anonyme, ce '
                'torrent ne trouvera aucun pair — choisissez « Direct ».',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.tertiary,
                ),
              ),
            ],
            const SizedBox(height: AppSpacing.md),
            TextField(
              controller: _destController,
              decoration: InputDecoration(
                labelText: 'Dossier de destination (optionnel)',
                prefixIcon: const Icon(Icons.folder_outlined),
                suffixIcon: IconButton(
                  tooltip: 'Parcourir…',
                  icon: const Icon(Icons.folder_open),
                  onPressed: () async {
                    final dir = await getDirectoryPath();
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
              title: const Text('Ajouter en pause'),
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
          child: const Text('Annuler'),
        ),
        FilledButton(
          onPressed: _canSubmit ? _submit : null,
          child: _busy
              ? const SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Text('Ajouter'),
        ),
      ],
    );
  }
}
