// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../l10n/l10n_ext.dart';
import '../design/design_tokens.dart';
import 'command_catalog.dart';

/// Palette de commandes Ctrl/Cmd+K (ADR-0021 §6) — recherche floue sur
/// le catalogue `buildCommandCatalog` (destinations, conversations,
/// sections de réglages, actions), navigation clavier ↑/↓/Entrée,
/// Échap pour fermer.
///
/// Ciblée clavier (desktop/web) : inatteignable sans clavier physique
/// — inoffensive sur les cibles tactiles.
class CommandPalette extends ConsumerStatefulWidget {
  const CommandPalette._();

  /// Ouvre la palette au-dessus de la route courante.
  static Future<void> show(BuildContext context) {
    return showDialog<void>(
      context: context,
      builder: (_) => const CommandPalette._(),
    );
  }

  @override
  ConsumerState<CommandPalette> createState() => _CommandPaletteState();
}

class _CommandPaletteState extends ConsumerState<CommandPalette> {
  final _controller = TextEditingController();
  final _searchNode = FocusNode();
  final _itemKeys = <String, GlobalKey>{};
  String _query = '';
  int _selected = 0;

  @override
  void initState() {
    super.initState();
    // `FocusNode.onKeyEvent` est consulté au niveau du nœud focalisé
    // avant la remontée aux ancêtres — interception fiable des flèches
    // malgré la gestion interne du curseur par le champ de saisie.
    _searchNode.onKeyEvent = _onKey;
  }

  @override
  void dispose() {
    _controller.dispose();
    _searchNode.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    if (event.logicalKey == LogicalKeyboardKey.arrowDown) {
      _move(1);
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.arrowUp) {
      _move(-1);
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.enter) {
      _runSelected();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  List<AppCommand> get _filtered =>
      _commands.where((c) => c.matches(_query)).toList();

  List<AppCommand> _commands = const [];

  void _move(int delta) {
    final items = _filtered;
    if (items.isEmpty) return;
    setState(() {
      _selected = (_selected + delta).clamp(0, items.length - 1);
    });
    final ctx = _itemKeys[items[_selected].id]?.currentContext;
    if (ctx != null) Scrollable.ensureVisible(ctx);
  }

  void _runSelected() {
    final items = _filtered;
    if (items.isEmpty) return;
    _run(items[_selected.clamp(0, items.length - 1).toInt()]);
  }

  /// Exécute puis ferme : `run` navigue (`ctx.go`) ou lit des providers
  /// — il doit s'exécuter tant que la palette (et son `ref`) est montée ;
  /// le dialogue route sur le navigateur racine, `go` seul ne le fermerait
  /// pas.
  void _run(AppCommand cmd) {
    cmd.run(context, ref);
    if (mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    _commands = buildCommandCatalog(context, ref);
    final items = _filtered;
    if (_selected >= items.length) {
      _selected = items.isEmpty ? 0 : items.length - 1;
    }

    // Regroupement dans l'ordre de `CommandGroup.values`.
    final rows = <Widget>[];
    var index = 0;
    for (final group in CommandGroup.values) {
      final grouped = items.where((c) => c.group == group).toList();
      if (grouped.isEmpty) continue;
      rows.add(_GroupHeader(group.label(l10n)));
      for (final cmd in grouped) {
        final i = index++;
        final key = _itemKeys.putIfAbsent(cmd.id, GlobalKey.new);
        rows.add(
          _CommandTile(
            key: key,
            command: cmd,
            selected: i == _selected,
            onTap: () => _run(cmd),
          ),
        );
      }
    }

    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () =>
            Navigator.of(context).pop(),
      },
      child: Dialog(
        backgroundColor: Colors.transparent,
        elevation: 0,
        insetPadding: const EdgeInsets.symmetric(
          horizontal: AppSpace.lg,
          vertical: AppSpace.xxl,
        ),
        clipBehavior: Clip.antiAlias,
        child: FrostedSurface(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 560, maxHeight: 440),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: _controller,
                  focusNode: _searchNode,
                  autofocus: true,
                  decoration: InputDecoration(
                    hintText: l10n.cmdPaletteHint,
                    prefixIcon: const Icon(Icons.keyboard_command_key),
                    border: InputBorder.none,
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: AppSpace.md,
                      vertical: AppSpace.md,
                    ),
                  ),
                  onChanged: (v) => setState(() {
                    _query = v;
                    _selected = 0;
                  }),
                ),
                const Divider(height: 1),
                Flexible(
                  child: rows.isEmpty
                      ? Padding(
                          padding: const EdgeInsets.all(AppSpace.xl),
                          child: Text(
                            l10n.cmdNoResults,
                            style: Theme.of(context).textTheme.bodyMedium
                                ?.copyWith(
                                  color: Theme.of(
                                    context,
                                  ).colorScheme.onSurfaceVariant,
                                ),
                          ),
                        )
                      : ListView(
                          shrinkWrap: true,
                          padding: const EdgeInsets.symmetric(
                            vertical: AppSpace.xs,
                          ),
                          children: rows,
                        ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _GroupHeader extends StatelessWidget {
  const _GroupHeader(this.label);

  final String label;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        AppSpace.md,
        AppSpace.sm,
        AppSpace.md,
        AppSpace.xs,
      ),
      child: Text(
        label.toUpperCase(),
        style: Theme.of(context).textTheme.labelSmall?.copyWith(
          letterSpacing: 1.2,
          color: Theme.of(context).colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

class _CommandTile extends StatelessWidget {
  const _CommandTile({
    super.key,
    required this.command,
    required this.selected,
    required this.onTap,
  });

  final AppCommand command;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: AppSpace.xs),
      child: ListTile(
        dense: true,
        selected: selected,
        selectedTileColor: theme.colorScheme.primaryContainer.withValues(
          alpha: 0.4,
        ),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(AppRadius.small),
        ),
        leading: Icon(command.icon, size: 18),
        title: Text(
          command.title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        trailing: command.subtitle == null
            ? null
            : Text(
                command.subtitle!,
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
        onTap: onTap,
      ),
    );
  }
}
