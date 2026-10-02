// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : détecteur de glisser-déposer HTML5 — listeners `dragenter`/
/// `dragover`/`dragleave`/`drop` sur le document. Les `File` du
/// `DataTransfer` n'ont pas de chemin : le contenu est lu par
/// `FileReader` en octets (même API que la variante native).
library;

import 'dart:async';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:web/web.dart' as web;

import '../platform/pick_file.dart';

/// Détecteur de glisser-déposer navigateur (même API que la variante
/// native).
class DropDetector extends StatefulWidget {
  const DropDetector({
    super.key,
    required this.onHover,
    required this.onFiles,
    required this.child,
  });

  /// Survol d'un dépôt (liseré visuel).
  final void Function(bool hovering) onHover;

  /// Fichiers lâchés (avant filtrage `.torrent`/`.magnet`).
  final void Function(List<PickedFile> files) onFiles;

  final Widget child;

  @override
  State<DropDetector> createState() => _DropDetectorState();
}

class _DropDetectorState extends State<DropDetector> {
  /// Profondeur de survol — `dragleave`/`dragenter` se déclenchent en
  /// entrant/sortant de chaque enfant du DOM : le liseré ne doit
  /// s'éteindre qu'à la sortie réelle de la page.
  int _depth = 0;
  bool _hovering = false;

  late final web.EventListener _onDragEnter = ((web.Event e) {
    e.preventDefault();
    _depth++;
    _setHover(true);
  }).toJS;
  late final web.EventListener _onDragOver = ((web.Event e) {
    // preventDefault obligatoire pour autoriser le `drop`.
    e.preventDefault();
  }).toJS;
  late final web.EventListener _onDragLeave = ((web.Event e) {
    e.preventDefault();
    if (--_depth <= 0) {
      _depth = 0;
      _setHover(false);
    }
  }).toJS;
  late final web.EventListener _onDrop = ((web.Event e) {
    e.preventDefault();
    _depth = 0;
    _setHover(false);
    final dt = (e as web.DragEvent).dataTransfer;
    if (dt == null) return;
    _readAll(dt);
  }).toJS;

  void _setHover(bool h) {
    if (_hovering != h) {
      _hovering = h;
      widget.onHover(h);
    }
  }

  /// Lit chaque `File` déposé en octets puis émet le lot.
  Future<void> _readAll(web.DataTransfer dt) async {
    final out = <PickedFile>[];
    final files = dt.files;
    for (var i = 0; i < files.length; i++) {
      final file = files.item(i);
      if (file == null) continue;
      out.add(PickedFile(name: file.name, bytes: await _read(file)));
    }
    if (out.isNotEmpty) widget.onFiles(out);
  }

  Future<Uint8List> _read(web.File file) {
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
      completer.complete(Uint8List(0));
    }).toJS;
    reader.readAsArrayBuffer(file);
    return completer.future;
  }

  @override
  void initState() {
    super.initState();
    web.document.addEventListener('dragenter', _onDragEnter);
    web.document.addEventListener('dragover', _onDragOver);
    web.document.addEventListener('dragleave', _onDragLeave);
    web.document.addEventListener('drop', _onDrop);
  }

  @override
  void dispose() {
    web.document.removeEventListener('dragenter', _onDragEnter);
    web.document.removeEventListener('dragover', _onDragOver);
    web.document.removeEventListener('dragleave', _onDragLeave);
    web.document.removeEventListener('drop', _onDrop);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
