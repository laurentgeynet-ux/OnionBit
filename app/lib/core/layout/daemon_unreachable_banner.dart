// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../config/connection_settings.dart';
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import '../theme/app_theme.dart';

/// Bannière « daemon injoignable » — visible tant que le SSE est
/// coupé. Si l'API répond mais rejette l'auth (401 : cas nominal web,
/// la page servie par le daemon n'a pas la clé), le message bascule
/// sur « clé API requise ». « Configurer… » ouvre le dialogue de
/// connexion (URL de base + clé API) ; « Relancer la découverte »
/// re-résout le daemon local.
class DaemonUnreachableBanner extends ConsumerWidget {
  const DaemonUnreachableBanner({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final scheme = Theme.of(context).colorScheme;
    final baseUrl = ref.watch(connectionSettingsProvider).value?.baseUrl ?? '';
    final unauthorized = ref.watch(apiUnauthorizedProvider).value ?? false;
    return Material(
      color: scheme.errorContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: AppSpacing.md,
          vertical: AppSpacing.xs,
        ),
        child: Row(
          children: [
            Icon(
              unauthorized ? Icons.key_off_outlined : Icons.cloud_off,
              size: 18,
              color: scheme.onErrorContainer,
            ),
            const SizedBox(width: AppSpacing.sm),
            Expanded(
              child: Text(
                unauthorized
                    ? context.l10n.daemonKeyRequired
                    : context.l10n.daemonUnreachableBanner(
                        baseUrl.isNotEmpty ? ' — $baseUrl' : '',
                      ),
                style: Theme.of(context).textTheme.bodySmall
                    ?.copyWith(color: scheme.onErrorContainer),
              ),
            ),
            // Re-découverte locale (processus voisin + configuration.json)
            // : sans objet sur web — la clé s'y saisit via Configurer.
            if (!kIsWeb) ...[
              TextButton.icon(
                onPressed: () =>
                    ref.read(connectionSettingsProvider.notifier).rediscover(),
                icon: const Icon(Icons.refresh, size: 16),
                label: Text(context.l10n.retry),
              ),
              const SizedBox(width: AppSpacing.xs),
            ],
            FilledButton.tonalIcon(
              onPressed: () => _ConnectionDialog.show(context),
              icon: const Icon(Icons.settings_outlined, size: 16),
              label: Text(context.l10n.configure),
            ),
          ],
        ),
      ),
    );
  }
}

/// Dialogue de première connexion : URL de base (`http://host:port`)
/// + clé API — même persistance que la section Réglages → Connexion.
class _ConnectionDialog extends ConsumerStatefulWidget {
  const _ConnectionDialog();

  static Future<void> show(BuildContext context) =>
      showDialog(context: context, builder: (_) => const _ConnectionDialog());

  @override
  ConsumerState<_ConnectionDialog> createState() => _ConnectionDialogState();
}

class _ConnectionDialogState extends ConsumerState<_ConnectionDialog> {
  final _url = TextEditingController();
  final _key = TextEditingController();
  bool _saving = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    final c = ref.read(connectionSettingsProvider).value;
    if (c != null) {
      _url.text = c.baseUrl;
      _key.text = c.apiKey;
    }
  }

  @override
  void dispose() {
    _url.dispose();
    _key.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final url = _url.text.trim();
    final uri = Uri.tryParse(url);
    if (uri == null || !uri.hasScheme || uri.host.isEmpty) {
      setState(() => _error = context.l10n.invalidUrl);
      return;
    }
    setState(() {
      _saving = true;
      _error = null;
    });
    await ref
        .read(connectionSettingsProvider.notifier)
        .save(baseUrl: url, apiKey: _key.text);
    // Re-résolution si loopback (daemon local relancé si mort).
    await ref.read(connectionSettingsProvider.notifier).rediscover();
    if (mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return AlertDialog(
      title: Text(l10n.connectionDialogTitle),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: _url,
              decoration: InputDecoration(
                labelText: l10n.daemonUrlLabel,
                hintText: 'http://127.0.0.1:8085',
                errorText: _error,
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: _key,
              obscureText: true,
              decoration: InputDecoration(labelText: l10n.apiKeyLabel),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: _saving ? null : _save,
          child: Text(_saving ? l10n.connecting : l10n.save),
        ),
      ],
    );
  }
}
