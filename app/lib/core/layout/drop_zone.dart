// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../di/providers.dart';
import '../platform/pick_file.dart';
import 'drop_zone_web.dart'
    if (dart.library.io) 'drop_zone_native.dart' as impl;

/// Zone de glisser-déposer globale : un `.torrent`/`.magnet`
/// lâché n'importe où sur la fenêtre rejoint la file
/// `pendingFilesProvider` (un dialogue « Ajouter » s'ouvre à
/// tour de rôle). Survol → liseré d'accent autour du contenu.
///
/// Le détecteur est spécifique plateforme : `desktop_drop` natif,
/// événements HTML5 `dragover`/`drop` sur web.
class DropZone extends ConsumerStatefulWidget {
  const DropZone({super.key, required this.child});

  final Widget child;

  @override
  ConsumerState<DropZone> createState() => _DropZoneState();
}

class _DropZoneState extends ConsumerState<DropZone> {
  bool _hovering = false;

  void _enqueue(List<PickedFile> files) {
    final kept = files
        .where(
          (f) =>
              f.name.toLowerCase().endsWith('.torrent') ||
              f.name.toLowerCase().endsWith('.magnet'),
        )
        .toList();
    if (kept.isNotEmpty) {
      ref.read(pendingFilesProvider.notifier).enqueue(kept);
    }
  }

  @override
  Widget build(BuildContext context) {
    return impl.DropDetector(
      onHover: (h) => setState(() => _hovering = h),
      onFiles: _enqueue,
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
