// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : sélection via `<input type="file">` éphémère — le `File`
/// navigateur n'a pas de chemin, le contenu est lu par `FileReader`.
library;

import 'dart:async';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

import 'pick_file.dart';

Future<Uint8List> readPickedBytes(PickedFile f) =>
    Future.value(f.bytes ?? Uint8List(0));

/// Lit un `File` navigateur en octets.
Future<Uint8List> _readFile(web.File file) {
  final completer = Completer<Uint8List>();
  final reader = web.FileReader();
  reader.onload = ((web.Event _) {
    final result = reader.result;
    completer.complete(
      result.isA<JSArrayBuffer>()
          ? (result! as JSArrayBuffer).toDart.asUint8List()
          : Uint8List(0),
    );
  }).toJS;
  reader.onerror = ((web.Event _) {
    completer.completeError(reader.error ?? web.DOMException('read error'));
  }).toJS;
  reader.readAsArrayBuffer(file);
  return completer.future;
}

Future<PickedFile?> pickTorrentFile() {
  final completer = Completer<PickedFile?>();
  final input =
      web.document.createElement('input') as web.HTMLInputElement
        ..type = 'file'
        ..accept = '.torrent,application/x-bittorrent';
  input.onchange = ((web.Event _) {
    final file = input.files?.item(0);
    if (file == null) {
      completer.complete(null);
      return;
    }
    _readFile(file)
        .then(
          (bytes) => completer.complete(PickedFile(name: file.name, bytes: bytes)),
        )
        .catchError((_) => completer.complete(null));
  }).toJS;
  // Fermeture de la boîte sans choix (Chrome ≥ 113).
  input.oncancel = ((web.Event _) => completer.complete(null)).toJS;
  input.click();
  return completer.future;
}
