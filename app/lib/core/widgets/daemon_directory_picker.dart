// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../api/api_client.dart';
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import '../design/design_tokens.dart';

/// Dialogue de choix d'un dossier **sur la machine du daemon**,
/// naviguant `GET /api/files/browse` (`current`, `paths[]`, `..` en
/// tête, séparateur fourni par le backend).
///
/// Utilisé sur web — un navigateur ne peut pas énumérer le système de
/// fichiers du daemon — et réutilisable partout où un chemin distant
/// est attendu (destination, watch folder, `move_storage`).
class DaemonDirectoryPicker extends ConsumerStatefulWidget {
  const DaemonDirectoryPicker({super.key, this.initialPath});

  /// Dossier de départ (`null`/`''` = `browse` sans path → cwd/racine).
  final String? initialPath;

  /// Ouvre le dialogue ; renvoie le chemin choisi ou `null`.
  static Future<String?> show(BuildContext context, {String? initialPath}) =>
      showDialog<String>(
        context: context,
        builder: (_) => DaemonDirectoryPicker(initialPath: initialPath),
      );

  @override
  ConsumerState<DaemonDirectoryPicker> createState() =>
      _DaemonDirectoryPickerState();
}

class _BrowseEntry {
  const _BrowseEntry({required this.name, required this.path});
  final String name;
  final String path;
}

class _DaemonDirectoryPickerState
    extends ConsumerState<DaemonDirectoryPicker> {
  String _current = '';
  List<_BrowseEntry> _entries = [];
  bool _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    _browse(widget.initialPath ?? '');
  }

  Future<void> _browse(String path) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final json = await ref
          .read(apiClientProvider)
          .get('/files/browse', query: {'path': path});
      if (!mounted) return;
      final paths = (json['paths'] as List? ?? [])
          .whereType<Map>()
          .where((e) => e['dir'] == true)
          .map(
            (e) => _BrowseEntry(
              name: '${e['name']}',
              path: '${e['path']}',
            ),
          )
          .toList();
      setState(() {
        _current = '${json['current']}';
        _entries = paths;
        _loading = false;
      });
    } on ApiException catch (e) {
      if (mounted) {
        setState(() {
          _error = e.message;
          _loading = false;
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = '$e';
          _loading = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    return AlertDialog(
      title: Text(l10n.dirPickerTitle),
      content: SizedBox(
        width: 480,
        height: 380,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.folder_outlined,
                  size: 16,
                  color: theme.colorScheme.outline,
                ),
                const SizedBox(width: AppSpace.xs),
                Expanded(
                  child: Text(
                    _current,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodySmall,
                  ),
                ),
              ],
            ),
            const SizedBox(height: AppSpace.sm),
            Expanded(
              child: _loading
                  ? const Center(child: CircularProgressIndicator())
                  : _error != null
                  ? Center(
                      child: Text(
                        _error!,
                        style: theme.textTheme.bodySmall?.copyWith(
                          color: theme.colorScheme.error,
                        ),
                      ),
                    )
                  : ListView.builder(
                      itemCount: _entries.length,
                      itemBuilder: (context, i) {
                        final e = _entries[i];
                        return ListTile(
                          dense: true,
                          leading: Icon(
                            e.name == '..'
                                ? Icons.drive_folder_upload_outlined
                                : Icons.folder_outlined,
                            size: 18,
                          ),
                          title: Text(
                            e.name,
                            overflow: TextOverflow.ellipsis,
                          ),
                          onTap: () => _browse(e.path),
                        );
                      },
                    ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: _loading || _current.isEmpty
              ? null
              : () => Navigator.of(context).pop(_current),
          child: Text(l10n.dirPickerSelect),
        ),
      ],
    );
  }
}
