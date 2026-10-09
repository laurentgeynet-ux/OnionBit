// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../design/design_tokens.dart';
import '../l10n/l10n_ext.dart';

/// Ouvre la page caméra plein écran et rend la première valeur de QR
/// détectée. `null` si l'utilisateur annule ou si la plateforme n'est
/// pas couverte par `mobile_scanner` (Windows/Linux — le bouton
/// d'entrée est d'ailleurs masqué sur ces cibles).
Future<String?> scanPairingQr(BuildContext context) async {
  if (!Platform.isAndroid && !Platform.isIOS && !Platform.isMacOS) {
    return null;
  }
  return Navigator.of(context, rootNavigator: true).push<String>(
    MaterialPageRoute(builder: (_) => const _ScannerPage()),
  );
}

class _ScannerPage extends StatefulWidget {
  const _ScannerPage();

  @override
  State<_ScannerPage> createState() => _ScannerPageState();
}

class _ScannerPageState extends State<_ScannerPage> {
  bool _done = false;

  void _onDetect(BarcodeCapture capture) {
    if (_done) return;
    for (final b in capture.barcodes) {
      final raw = b.rawValue;
      if (raw != null && raw.isNotEmpty) {
        _done = true;
        Navigator.of(context).pop(raw);
        return;
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
        title: Text(l10n.pairScanTitle),
      ),
      body: Stack(
        fit: StackFit.expand,
        children: [
          MobileScanner(onDetect: _onDetect),
          // Cadre de visée — repère visuel purement décoratif.
          Center(
            child: Container(
              width: 220,
              height: 220,
              decoration: BoxDecoration(
                border: Border.all(
                  color: Theme.of(context).colorScheme.primary,
                  width: 2,
                ),
                borderRadius: BorderRadius.circular(AppRadius.large),
              ),
            ),
          ),
          Positioned(
            left: 0,
            right: 0,
            bottom: AppSpace.xl,
            child: Text(
              l10n.pairScanHint,
              textAlign: TextAlign.center,
              style: Theme.of(
                context,
              ).textTheme.bodyMedium?.copyWith(color: Colors.white),
            ),
          ),
        ],
      ),
    );
  }
}
