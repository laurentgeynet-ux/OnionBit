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
import '../../../../core/platform/pick_directory.dart';
import '../../../../core/platform/pick_files.dart';
import '../../../../core/widgets/empty_state.dart';
import '../../../../core/widgets/error_state.dart';

/// Page « Zone publique » — explorateur complet des fichiers en
/// clair du device sous `data/public/` (ADR-0018) : navigation
/// `GET /api/files/browse?path=@public&files=1`, sélection
/// multiple, copier/couper/coller internes (`/files/copy|move`),
/// suppression avec confirmation (pas de corbeille), nouveau
/// dossier (`/files/mkdir`), renommage (`/files/move`) et import
/// de fichiers/dossiers locaux (`/files/copy` — la source vit hors
/// zone, la cible reste confinée `@public`). Double-clic/tap sur
/// un fichier l'ouvre via le shell natif.
class PublicZonePage extends ConsumerStatefulWidget {
  const PublicZonePage({super.key});

  @override
  ConsumerState<PublicZonePage> createState() => _PublicZonePageState();
}

/// Presse-papiers interne à la page — `cut` = déplacement
/// (`move`), sinon copie (`copy`).
class _Clipboard {
  const _Clipboard(this.items, {required this.cut});
  final List<_Entry> items;
  final bool cut;
}

class _PublicZonePageState extends ConsumerState<PublicZonePage> {
  /// Racine publique absolue résolue par le daemon (premier `current`
  /// renvoyé pour `@public`) — borne la remontée `..`.
  String? _root;
  String _current = '';
  String _separator = '/';
  List<_Entry> _entries = [];
  final Set<String> _selected = {};
  _Clipboard? _clipboard;
  bool _loading = true;
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _browse('@public');
  }

  ApiClient get _api => ref.read(apiClientProvider);

  Future<void> _browse(String path) async {
    setState(() {
      _loading = true;
      _error = null;
      _selected.clear();
    });
    try {
      final json = await _api.get(
        '/files/browse',
        query: {'path': path, 'files': '1'},
      );
      if (!mounted) return;
      setState(() {
        _current = '${json['current']}';
        _separator = '${json['separator'] ?? '/'}';
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

  List<_Entry> get _sel =>
      _entries.where((e) => _selected.contains(e.path)).toList();

  String _join(String dir, String name) =>
      dir.endsWith(_separator) ? '$dir$name' : '$dir$_separator$name';

  String _basename(String path) {
    final norm = path.replaceAll('\\', '/');
    final i = norm.lastIndexOf('/');
    return i < 0 ? norm : norm.substring(i + 1);
  }

  void _toast(String msg) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(msg)));
  }

  /// Exécute une mutation puis rafraîchit ; l'erreur API remonte
  /// en toast (409 cible existante, 403 confinement, etc.).
  Future<void> _mutate(Future<void> Function() op) async {
    setState(() => _busy = true);
    try {
      await op();
    } on ApiException catch (e) {
      _toast(e.message);
    } catch (e) {
      _toast('$e');
    }
    if (mounted) {
      setState(() => _busy = false);
      await _browse(_current);
    }
  }

  // -- Actions --

  Future<void> _newFolder() async {
    final l10n = context.l10n;
    final name = await _prompt(l10n.explorerNewFolder, l10n.explorerNameHint);
    if (name == null || name.isEmpty) return;
    await _mutate(
      () => _api.post(
        '/files/mkdir',
        body: {'path': _join(_current, name)},
      ),
    );
  }

  Future<void> _rename(_Entry e) async {
    final l10n = context.l10n;
    final name = await _prompt(
      l10n.explorerRename,
      l10n.explorerNameHint,
      initial: e.name,
    );
    if (name == null || name.isEmpty || name == e.name) return;
    final parent = _current;
    await _mutate(
      () => _api.post(
        '/files/move',
        body: {'src': e.path, 'dst': _join(parent, name)},
      ),
    );
  }

  Future<void> _delete() async {
    final l10n = context.l10n;
    final items = _sel;
    if (items.isEmpty) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        icon: const Icon(Icons.delete_forever_outlined),
        title: Text(l10n.explorerDeleteTitle),
        content: Text(l10n.explorerDeleteMessage(items.length)),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text(l10n.explorerDelete),
          ),
        ],
      ),
    );
    if (ok != true) return;
    await _mutate(
      () => _api.post(
        '/files/delete',
        body: {
          'paths': [for (final e in items) e.path],
        },
      ),
    );
  }

  Future<void> _paste() async {
    final clip = _clipboard;
    if (clip == null || clip.items.isEmpty) return;
    await _mutate(() async {
      for (final e in clip.items) {
        await _api.post(
          clip.cut ? '/files/move' : '/files/copy',
          body: {'src': e.path, 'dst': _join(_current, e.name)},
        );
      }
      if (clip.cut) {
        _clipboard = null;
      }
    });
  }

  Future<void> _importFiles() async {
    final paths = await pickDaemonFiles(context);
    if (paths.isEmpty || !mounted) return;
    await _mutate(() async {
      for (final p in paths) {
        await _api.post(
          '/files/copy',
          body: {'src': p, 'dst': _join(_current, _basename(p))},
        );
      }
    });
  }

  Future<void> _importDir() async {
    final dir = await pickDaemonDirectory(context);
    if (dir == null || !mounted) return;
    await _mutate(
      () => _api.post(
        '/files/copy',
        body: {'src': dir, 'dst': _join(_current, _basename(dir))},
      ),
    );
  }

  Future<String?> _prompt(String title, String hint, {String? initial}) {
    final ctrl = TextEditingController(text: initial);
    return showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title),
        content: TextField(
          controller: ctrl,
          autofocus: true,
          decoration: InputDecoration(hintText: hint),
          onSubmitted: (v) => Navigator.of(ctx).pop(v.trim()),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: Text(context.l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(ctrl.text.trim()),
            child: Text(context.l10n.ok),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    final atRoot = _current.isEmpty || _current == _root;
    final hasSel = _sel.isNotEmpty;
    final canPaste = _clipboard?.items.isNotEmpty == true;
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
                onPressed: _busy ? null : () => _browse(_current),
                icon: const Icon(Icons.refresh, size: 20),
              ),
            ],
          ),
        ),
        // Barre d'outils explorateur — actions de mutation sur la
        // sélection ou le presse-papiers interne.
        Padding(
          padding: const EdgeInsets.fromLTRB(
            AppSpace.md,
            0,
            AppSpace.md,
            AppSpace.sm,
          ),
          child: Wrap(
            spacing: AppSpace.xs,
            runSpacing: AppSpace.xs,
            children: [
              _ToolButton(
                icon: Icons.create_new_folder_outlined,
                label: l10n.explorerNewFolder,
                onPressed: _busy ? null : _newFolder,
              ),
              _ToolButton(
                icon: Icons.input_outlined,
                label: l10n.explorerImportFiles,
                onPressed: _busy ? null : _importFiles,
              ),
              _ToolButton(
                icon: Icons.drive_folder_upload_outlined,
                label: l10n.explorerImportDir,
                onPressed: _busy ? null : _importDir,
              ),
              const SizedBox(width: AppSpace.sm),
              _ToolButton(
                icon: Icons.content_cut_outlined,
                label: l10n.explorerCut,
                onPressed: !hasSel || _busy
                    ? null
                    : () => setState(
                        () => _clipboard = _Clipboard(_sel, cut: true),
                      ),
              ),
              _ToolButton(
                icon: Icons.copy_outlined,
                label: l10n.explorerCopy,
                onPressed: !hasSel || _busy
                    ? null
                    : () => setState(
                        () => _clipboard = _Clipboard(_sel, cut: false),
                      ),
              ),
              _ToolButton(
                icon: Icons.content_paste_outlined,
                label: l10n.explorerPaste,
                onPressed: !canPaste || _busy ? null : _paste,
              ),
              _ToolButton(
                icon: Icons.drive_file_rename_outline,
                label: l10n.explorerRename,
                onPressed: _sel.length != 1 || _busy
                    ? null
                    : () => _rename(_sel.first),
              ),
              _ToolButton(
                icon: Icons.delete_outline,
                label: l10n.explorerDelete,
                onPressed: !hasSel || _busy ? null : _delete,
                danger: true,
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
                    if (e.name == '..') {
                      return ListTile(
                        dense: true,
                        leading: const Icon(
                          Icons.drive_folder_upload_outlined,
                          size: 20,
                        ),
                        title: const Text('..'),
                        onTap: () => _browse(e.path),
                      );
                    }
                    return ListTile(
                      dense: true,
                      leading: SizedBox(
                        width: 24,
                        child: Checkbox(
                          value: _selected.contains(e.path),
                          onChanged: _busy
                              ? null
                              : (v) => setState(() {
                                  if (v == true) {
                                    _selected.add(e.path);
                                  } else {
                                    _selected.remove(e.path);
                                  }
                                }),
                        ),
                      ),
                      title: Row(
                        children: [
                          Icon(
                            e.isDir
                                ? Icons.folder_outlined
                                : Icons.insert_drive_file_outlined,
                            size: 20,
                          ),
                          const SizedBox(width: AppSpace.xs),
                          Expanded(
                            child: Text(
                              e.name,
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                        ],
                      ),
                      trailing: e.isDir
                          ? null
                          : const Icon(Icons.open_in_new, size: 16),
                      onTap: () =>
                          e.isDir ? _browse(e.path) : openPath(e.path),
                      onLongPress: () => setState(
                        () => _selected.add(e.path),
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class _ToolButton extends StatelessWidget {
  const _ToolButton({
    required this.icon,
    required this.label,
    required this.onPressed,
    this.danger = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onPressed;
  final bool danger;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return TextButton.icon(
      onPressed: onPressed,
      icon: Icon(
        icon,
        size: 18,
        color: danger && onPressed != null
            ? theme.colorScheme.error
            : null,
      ),
      label: Text(
        label,
        style: danger && onPressed != null
            ? TextStyle(color: theme.colorScheme.error)
            : null,
      ),
      style: TextButton.styleFrom(
        padding: const EdgeInsets.symmetric(horizontal: AppSpace.sm),
        visualDensity: VisualDensity.compact,
      ),
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
