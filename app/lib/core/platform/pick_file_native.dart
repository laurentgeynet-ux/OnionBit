// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Desktop (`dart:io`) : sélecteur natif `file_selector` ; le chemin
/// est conservé (argv/drop travaillent aussi en chemins).
library;

import 'dart:io';
import 'dart:typed_data';

import 'package:file_selector/file_selector.dart';

import 'pick_file.dart';

Future<Uint8List> readPickedBytes(PickedFile f) {
  final bytes = f.bytes;
  if (bytes != null) return Future.value(bytes);
  return File(f.path!).readAsBytes();
}

Future<PickedFile?> pickTorrentFile() async {
  final file = await openFile(
    acceptedTypeGroups: [
      const XTypeGroup(label: 'torrent', extensions: ['torrent']),
    ],
  );
  if (file == null) return null;
  return PickedFile(
    name: file.name,
    path: file.path,
    bytes: await file.readAsBytes(),
  );
}
