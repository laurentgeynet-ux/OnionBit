// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../design/design_tokens.dart';
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import 'lan_addresses.dart';
import 'pairing_api.dart';
import 'pairing_payload.dart';

/// Feuille « appairer un mobile » (ADR-0021 §8, étape 76) : émet un
/// jeton à usage unique côté daemon (`POST /api/pairing/token`) et
/// l'affiche en QR avec `host:port` joignables depuis le mobile.
/// L'hôte par défaut est celui de la connexion courante — s'il est
/// loopback, la première adresse LAN IPv4 est proposée (le daemon
/// doit alors écouter hors loopback, `api.http_host`).
Future<void> showPairingSheet(BuildContext context) {
  return showModalBottomSheet(
    context: context,
    isScrollControlled: true,
    showDragHandle: true,
    builder: (_) => const Padding(
      padding: EdgeInsets.fromLTRB(
        AppSpace.lg,
        0,
        AppSpace.lg,
        AppSpace.lg,
      ),
      child: PairingSheet(),
    ),
  );
}

class PairingSheet extends ConsumerStatefulWidget {
  const PairingSheet({super.key});

  @override
  ConsumerState<PairingSheet> createState() => _PairingSheetState();
}

class _PairingSheetState extends ConsumerState<PairingSheet> {
  final _hostController = TextEditingController();
  PairingGrant? _grant;
  int _port = 0;
  int _secsLeft = 0;
  String? _error;
  Timer? _ticker;
  List<String> _lanIps = const [];
  bool _hostInitialized = false;

  @override
  void initState() {
    super.initState();
    _hostController.addListener(() => setState(() {}));
    _loadLanIps();
    _issue();
  }

  @override
  void dispose() {
    _ticker?.cancel();
    _hostController.dispose();
    super.dispose();
  }

  /// Adresses LAN du poste — proposées quand la connexion courante
  /// est en loopback (le mobile ne peut pas joindre 127.0.0.1). Si le
  /// champ hôte est resté loopback, la première adresse LAN le
  /// remplace (les interfaces arrivent après `initState`).
  Future<void> _loadLanIps() async {
    final ips = await lanIpv4Addresses();
    if (!mounted) return;
    setState(() {
      _lanIps = ips;
      if (_isLoopback(_hostController.text.trim()) && ips.isNotEmpty) {
        _hostController.text = ips.first;
      }
    });
  }

  Future<void> _issue() async {
    setState(() {
      _grant = null;
      _error = null;
    });
    try {
      final grant = await const PairingApi().issue(
        ref.read(apiClientProvider),
      );
      if (!mounted) return;
      setState(() {
        _grant = grant;
        _secsLeft = grant.expiresInSecs;
      });
      _ticker?.cancel();
      _ticker = Timer.periodic(const Duration(seconds: 1), (t) {
        if (!mounted || _secsLeft <= 1) {
          t.cancel();
          if (mounted && _secsLeft <= 1) setState(() => _secsLeft = 0);
          return;
        }
        setState(() => _secsLeft -= 1);
      });
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    }
  }

  /// Initialise hôte/port depuis la connexion courante — une fois
  /// (les saisies suivantes sont celles de l'utilisateur).
  void _initHost(String baseUrl) {
    if (_hostInitialized) return;
    _hostInitialized = true;
    final uri = Uri.tryParse(baseUrl);
    _port = uri?.port ?? 0;
    _hostController.text = uri?.host ?? '';
  }

  bool get _loopbackHost => _isLoopback(_hostController.text.trim());

  static bool _isLoopback(String host) =>
      host == '127.0.0.1' || host == 'localhost' || host == '::1';

  PairingPayload? get _payload {
    final grant = _grant;
    final host = _hostController.text.trim();
    if (grant == null || host.isEmpty || _port <= 0) return null;
    return PairingPayload(host: host, port: _port, token: grant.token);
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final baseUrl = ref.watch(appConfigProvider).baseUrl;
    _initHost(baseUrl);

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(l10n.pairTitle, style: theme.textTheme.titleLarge),
        const SizedBox(height: AppSpace.sm),
        Text(l10n.pairBody, style: theme.textTheme.bodyMedium),
        const SizedBox(height: AppSpace.md),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(
              child: TextField(
                controller: _hostController,
                decoration: InputDecoration(
                  labelText: l10n.pairHostLabel,
                  helperText: l10n.pairHostHelper,
                  prefixIcon: const Icon(Icons.lan_outlined, size: 18),
                  border: const OutlineInputBorder(),
                  isDense: true,
                ),
              ),
            ),
            if (_lanIps.isNotEmpty) ...[
              const SizedBox(width: AppSpace.sm),
              PopupMenuButton<String>(
                tooltip: l10n.pairLanAddresses,
                icon: const Icon(Icons.arrow_drop_down),
                onSelected: (ip) => _hostController.text = ip,
                itemBuilder: (_) => [
                  for (final ip in _lanIps)
                    PopupMenuItem(value: ip, child: Text(ip)),
                ],
              ),
            ],
          ],
        ),
        if (_loopbackHost) ...[
          const SizedBox(height: AppSpace.sm),
          Row(
            children: [
              Icon(
                Icons.warning_amber,
                size: 16,
                color: context.semanticColors.warning,
              ),
              const SizedBox(width: AppSpace.xs),
              Expanded(
                child: Text(
                  l10n.pairLoopbackWarning,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: context.semanticColors.warning,
                  ),
                ),
              ),
            ],
          ),
        ],
        const SizedBox(height: AppSpace.md),
        Center(
          child: _error != null
              ? Column(
                  children: [
                    Text(
                      _error!,
                      style: TextStyle(color: scheme.error),
                      textAlign: TextAlign.center,
                    ),
                    const SizedBox(height: AppSpace.sm),
                    FilledButton.tonal(
                      onPressed: _issue,
                      child: Text(l10n.retry),
                    ),
                  ],
                )
              : _payload == null
              ? const Padding(
                  padding: EdgeInsets.all(AppSpace.xl),
                  child: CircularProgressIndicator(),
                )
              : Column(
                  children: [
                    Container(
                      padding: const EdgeInsets.all(AppSpace.sm),
                      decoration: BoxDecoration(
                        color: Colors.white,
                        borderRadius: BorderRadius.circular(
                          AppRadius.medium,
                        ),
                      ),
                      child: QrImageView(
                        data: _payload!.encode(),
                        size: 200,
                      ),
                    ),
                    const SizedBox(height: AppSpace.sm),
                    Text(
                      _secsLeft > 0
                          ? l10n.pairExpiresIn(_secsLeft)
                          : l10n.pairExpired,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: _secsLeft > 0
                            ? scheme.onSurfaceVariant
                            : context.semanticColors.warning,
                      ),
                    ),
                    TextButton.icon(
                      onPressed: _issue,
                      icon: const Icon(Icons.refresh, size: 16),
                      label: Text(l10n.pairRegenerate),
                    ),
                  ],
                ),
        ),
      ],
    );
  }
}
