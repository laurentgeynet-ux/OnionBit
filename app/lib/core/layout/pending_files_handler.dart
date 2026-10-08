// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../features/downloads/presentation/widgets/add_download_dialog.dart';
import '../di/providers.dart';
import '../platform/pick_file.dart';

/// Ouvre le dialogue « Ajouter » pour chaque fichier de
/// `pendingFilesProvider` (argv « Ouvrir avec », glisser-déposer)
/// — un fichier à la fois, le suivant quand le dialogue se ferme.
/// Ne rend rien.
class PendingFilesHandler extends ConsumerStatefulWidget {
  const PendingFilesHandler({super.key});

  @override
  ConsumerState<PendingFilesHandler> createState() =>
      _PendingFilesHandlerState();
}

class _PendingFilesHandlerState extends ConsumerState<PendingFilesHandler> {
  bool _dialogOpen = false;

  Future<void> _openNext() async {
    if (_dialogOpen) return;
    final file = ref.read(pendingFilesProvider).firstOrNull;
    if (file == null) return;
    _dialogOpen = true;
    await AddDownloadDialog.show(context, initialFile: file);
    _dialogOpen = false;
    if (!mounted) return;
    ref.read(pendingFilesProvider.notifier).pop();
    // Post-frame : laisse le dialogue précédent se fermer avant
    // d'ouvrir le suivant de la file.
    WidgetsBinding.instance.addPostFrameCallback((_) => _openNext());
  }

  @override
  Widget build(BuildContext context) {
    ref.listen<List<PickedFile>>(pendingFilesProvider, (prev, next) {
      if (next.isNotEmpty) {
        WidgetsBinding.instance.addPostFrameCallback((_) => _openNext());
      }
    });
    // Cas argv : la file est deja remplie au premier build.
    if (!_dialogOpen && ref.read(pendingFilesProvider).isNotEmpty) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _openNext());
    }
    return const SizedBox.shrink();
  }
}
