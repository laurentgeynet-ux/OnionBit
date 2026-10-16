// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../core/di/providers.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/platform/desktop_shell.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';

/// Page « Zone publique » — explorateur des fichiers en clair du
/// device sous `data/public/` (ADR-0018). Navigue
/// `GET /api/files/browse?path=@public&files=1` : dossiers et
/// fichiers, remontée `..` bornée à la racine publique (la zone
/// privée n'est de toute façon pas navigable — `refuse_private`).
/// Double-clic/tap sur un fichier l'ouvre via le shell natif.
class PublicZonePage extends ConsumerStatefulWidget {
  const PublicZonePage({super.key});

  @override
  ConsumerState<PublicZonePage> createState() => _PublicZonePageState();
}

class _PublicZonePageState extends ConsumerState<PublicZonePage> {
  /// Racine publique absolue résolue par le daemon (premier `current`
  /// renvoyé pour `@public`) — borne la remontée `..`.
  String? _root;
  String _current = '';
  List<_Entry> _entries = [];
  bool _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    _browse('@public');
  }

  Future<void> _browse(String path) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final json = await ref.read(apiClientProvider).get(
        '/files/browse',
        query: {'path': path, 'files': '1'},
      );
      if (!mounted) return;
      setState(() {
        _current = '${json['current']}';
        _root ??= _current;
        _entries = [
          for (final e in (json['paths'] as List? ?? []).whereType<Map>())
            _Entry(
              name: '${e['name']}',
              path: '${e['path']}',
              isDir: e['dir'] == true,
            ),
        ];
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
    final atRoot = _current.isEmpty || _current == _root;
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
                  l10n.publicZoneTitle,
                  style: theme.textTheme.titleSmall,
                ),
              ),
              Tooltip(
                message: l10n.openFolder,
                child: IconButton(
                  icon: const Icon(Icons.folder_open, size: 20),
                  onPressed: _current.isEmpty
                      ? null
                      : () => openPath(_current),
                ),
              ),
              IconButton(
                tooltip: l10n.refresh,
                onPressed: () => _browse(_current),
                icon: const Icon(Icons.refresh, size: 20),
              ),
            ],
          ),
        ),
        // Chemin courant — le spec `@public` est remplacé par le
        // chemin absolu résolu du daemon dès la première réponse.
        Padding(
          padding: const EdgeInsets.fromLTRB(
            AppSpace.md,
            0,
            AppSpace.md,
            AppSpace.sm,
          ),
          child: Row(
            children: [
              Icon(
                Icons.folder_outlined,
                size: 16,
                color: theme.colorScheme.outline,
              ),
              const SizedBox(width: AppSpace.xs),
              Expanded(
                child: Text(
                  _current.isEmpty ? '…' : _current,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ),
            ],
          ),
        ),
        Expanded(
          child: _loading
              ? const Center(child: CircularProgressIndicator())
              : _error != null
              ? ErrorState(
                  error: _error!,
                  onRetry: () => _browse(_current.isEmpty ? '@public' : _current),
                )
              : _entries.where((e) => e.name != '..' || !atRoot).isEmpty
              ? EmptyState(
                  icon: Icons.folder_off_outlined,
                  title: l10n.publicZoneEmptyTitle,
                  message: l10n.publicZoneEmptyMessage,
                )
              : ListView.builder(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpace.sm,
                  ),
                  itemCount: _entries.length,
                  itemBuilder: (context, i) {
                    final e = _entries[i];
                    // `..` à la racine publique : on ne sort pas de la
                    // zone (le listing resterait cohérent avec le
                    // titre de la page).
                    if (e.name == '..' && atRoot) {
                      return const SizedBox.shrink();
                    }
                    return ListTile(
                      dense: true,
                      leading: Icon(
                        e.name == '..'
                            ? Icons.drive_folder_upload_outlined
                            : e.isDir
                            ? Icons.folder_outlined
                            : Icons.insert_drive_file_outlined,
                        size: 20,
                      ),
                      title: Text(
                        e.name,
                        overflow: TextOverflow.ellipsis,
                      ),
                      trailing: e.isDir
                          ? null
                          : const Icon(Icons.open_in_new, size: 16),
                      onTap: () =>
                          e.isDir ? _browse(e.path) : openPath(e.path),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class _Entry {
  const _Entry({
    required this.name,
    required this.path,
    required this.isDir,
  });

  final String name;
  final String path;
  final bool isDir;
}
