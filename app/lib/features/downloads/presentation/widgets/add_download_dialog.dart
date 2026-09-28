import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/theme/app_theme.dart';
import '../providers/downloads_providers.dart';

/// Dialogue « Ajouter » — magnet/URI, fichier `.torrent` (binaire,
/// fonctionne aussi sur web), destination optionnelle et choix
/// d'anonymat binaire+sauts (règle backend : `anon_hops>0` exige
/// `safe_seeding`, envoyé automatiquement).
class AddDownloadDialog extends ConsumerStatefulWidget {
  const AddDownloadDialog({super.key, this.initialUri});

  /// URI pré-remplie (ex. magnet d'un résultat de recherche).
  final String? initialUri;

  static Future<void> show(BuildContext context, {String? initialUri}) =>
      showDialog(
        context: context,
        builder: (_) => AddDownloadDialog(initialUri: initialUri),
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
  int _hops = 0;
  bool _paused = false;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _uriController.dispose();
    _destController.dispose();
    super.dispose();
  }

  bool get _canSubmit =>
      !_busy && (_uriController.text.trim().isNotEmpty || _file != null);

  Future<void> _pickFile() async {
    final file = await openFile(
      acceptedTypeGroups: [
        const XTypeGroup(label: 'torrent', extensions: ['torrent']),
      ],
    );
    if (file != null) setState(() => _file = file);
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

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
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
                    onPressed: () => setState(() => _file = null),
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
