// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Sélection d'un fichier `.torrent`, spécifique plateforme.
///
/// Sur desktop (`dart.library.io`) : sélecteur natif `file_selector`.
/// Sur web : `<input type="file">` via `package:web` — le `File`
/// navigateur n'a pas de chemin exploitable, seuls le nom et les
/// octets remontent (suffisant : `ApiClient.putTorrent` envoie des
/// octets).
library;

import 'dart:typed_data';

import 'pick_file_web.dart'
    if (dart.library.io) 'pick_file_native.dart' as impl;

/// Fichier fourni par l'utilisateur (picker, argv, glisser-déposer).
/// Sur desktop `path` est renseigné ; sur web seuls `name` et `bytes`
/// existent.
class PickedFile {
  const PickedFile({required this.name, this.path, this.bytes});

  /// Nom affiché (`file.name` web, segment final du chemin desktop).
  final String name;

  /// Chemin local — desktop uniquement (argv, drop, picker natif).
  final String? path;

  /// Contenu — renseigné par le picker/drop web, peut être `null`
  /// quand `path` suffit (lecture différée via [readBytes]).
  final Uint8List? bytes;

  /// Lit le contenu (octets web, fichier `path` desktop).
  Future<Uint8List> readBytes() => impl.readPickedBytes(this);
}

/// Ouvre le sélecteur `.torrent` de la plateforme.
/// `null` = annulé par l'utilisateur.
Future<PickedFile?> pickTorrentFile() => impl.pickTorrentFile();
