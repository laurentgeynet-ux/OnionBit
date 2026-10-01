// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../di/providers.dart';

/// Zone de glisser-déposer globale : un `.torrent`/`.magnet`
/// lâché n'importe où sur la fenêtre rejoint la file
/// `pendingFilesProvider` (un dialogue « Ajouter » s'ouvre à
/// tour de rôle). Survol → liseré d'accent autour du contenu.
class DropZone extends ConsumerStatefulWidget {
  const DropZone({super.key, required this.child});

  final Widget child;

  @override
  ConsumerState<DropZone> createState() => _DropZoneState();
}

class _DropZoneState extends ConsumerState<DropZone> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    return DropTarget(
      onDragEntered: (_) => setState(() => _hovering = true),
      onDragExited: (_) => setState(() => _hovering = false),
      onDragDone: (details) {
        setState(() => _hovering = false);
        final paths = details.files
            .map((f) => f.path)
            .where(
              (p) =>
                  p.toLowerCase().endsWith('.torrent') ||
                  p.toLowerCase().endsWith('.magnet'),
            )
            .toList();
        if (paths.isNotEmpty) {
          ref.read(pendingFilesProvider.notifier).enqueue(paths);
        }
      },
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        foregroundDecoration: _hovering
            ? BoxDecoration(
                border: Border.all(
                  color: Theme.of(context).colorScheme.primary,
                  width: 2,
                ),
              )
            : null,
        child: widget.child,
      ),
    );
  }
}
