// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/config/connection_settings.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/pairing/pairing_api.dart';
import '../../../../core/pairing/pairing_payload.dart';
import '../../../../core/pairing/pairing_sheet.dart';
import '../../../../core/pairing/qr_scanner.dart';
import '../../../../core/pairing/scan_support.dart';

/// Section « Connexion daemon » — URL de base de `onionbit-api` + clé
/// éventuelle, persistées (`connectionSettingsProvider`) ; appairage
/// mobile par QR (ADR-0021 §8) : afficher le QR côté hôte, scanner +
/// redeem côté mobile.
class ConnectionSection extends ConsumerStatefulWidget {
  const ConnectionSection({super.key});

  @override
  ConsumerState<ConnectionSection> createState() => _ConnectionSectionState();
}

class _ConnectionSectionState extends ConsumerState<ConnectionSection> {
  final _urlController = TextEditingController();
  final _keyController = TextEditingController();
  bool _initialized = false;
  bool _pairingBusy = false;

  /// `mobile_scanner` couvre Android/iOS/macOS — autres cibles :
  /// le bouton de scan est masqué (`canScanPairingQr`).
  static bool get _canScan => canScanPairingQr;

  /// Le téléphone est la télécommande : il scanne le QR du poste qui
  /// héberge le daemon — il n'affiche pas de QR lui-même.
  static bool get _canShowQr => !isMobileRemote;

  @override
  void dispose() {
    _urlController.dispose();
    _keyController.dispose();
    super.dispose();
  }

  /// Scan → `redeem` → la clé API rendue est persistée avec
  /// l'adresse du QR (`connectionSettingsProvider` → l'app entière
  /// rebascule sur le daemon appairé).
  Future<void> _scanAndPair() async {
    final l10n = context.l10n;
    final raw = await scanPairingQr(context);
    if (!mounted || raw == null) return;
    final payload = PairingPayload.tryParse(raw);
    if (payload == null) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(l10n.pairInvalidQr)));
      return;
    }
    setState(() => _pairingBusy = true);
    try {
      final key = await const PairingApi().redeem(
        payload.baseUrl,
        payload.token,
      );
      await ref
          .read(connectionSettingsProvider.notifier)
          .save(baseUrl: payload.baseUrl, apiKey: key);
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(l10n.pairSuccess)));
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(l10n.pairFailed('$e'))));
      }
    } finally {
      if (mounted) setState(() => _pairingBusy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final settings = ref.watch(connectionSettingsProvider);

    return Card(
      margin: const EdgeInsets.all(AppSpace.md),
      child: Padding(
        padding: const EdgeInsets.all(AppSpace.md),
        child: settings.when(
          loading: () => const Center(child: CircularProgressIndicator()),
          error: (e, _) => Text('$e'),
          data: (cfg) {
            if (!_initialized) {
              _urlController.text = cfg.baseUrl;
              _keyController.text = cfg.apiKey;
              _initialized = true;
            }
            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('Daemon', style: theme.textTheme.titleMedium),
                const SizedBox(height: AppSpace.sm),
                TextField(
                  controller: _urlController,
                  decoration: InputDecoration(
                    labelText: l10n.apiUrlLabel,
                    hintText: l10n.defaultHint('http://127.0.0.1:8085'),
                    prefixIcon: const Icon(Icons.dns_outlined),
                  ),
                ),
                const SizedBox(height: AppSpace.sm),
                TextField(
                  controller: _keyController,
                  obscureText: true,
                  decoration: InputDecoration(
                    labelText: l10n.apiKeyLabelShort,
                    hintText: l10n.apiKeyHint,
                    prefixIcon: const Icon(Icons.key_outlined),
                  ),
                ),
                const SizedBox(height: AppSpace.sm),
                Wrap(
                  spacing: AppSpace.sm,
                  runSpacing: AppSpace.xs,
                  alignment: WrapAlignment.end,
                  children: [
                    if (_canScan)
                      OutlinedButton.icon(
                        onPressed: _pairingBusy ? null : _scanAndPair,
                        icon: const Icon(
                          Icons.qr_code_scanner,
                          size: 18,
                        ),
                        label: Text(l10n.pairScanQr),
                      ),
                    if (_canShowQr)
                      OutlinedButton.icon(
                        onPressed: () => showPairingSheet(context),
                        icon: const Icon(Icons.qr_code_2, size: 18),
                        label: Text(l10n.pairShowQr),
                      ),
                    FilledButton.tonal(
                      onPressed: () => ref
                          .read(connectionSettingsProvider.notifier)
                          .save(
                            baseUrl: _urlController.text,
                            apiKey: _keyController.text,
                          ),
                      child: Text(l10n.apply),
                    ),
                  ],
                ),
              ],
            );
          },
        ),
      ),
    );
  }
}
