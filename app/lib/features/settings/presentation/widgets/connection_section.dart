// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/config/connection_settings.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';

/// Section « Connexion daemon » — URL de base de `onionbit-api` + clé
/// éventuelle, persistées (`connectionSettingsProvider`).
class ConnectionSection extends ConsumerStatefulWidget {
  const ConnectionSection({super.key});

  @override
  ConsumerState<ConnectionSection> createState() => _ConnectionSectionState();
}

class _ConnectionSectionState extends ConsumerState<ConnectionSection> {
  final _urlController = TextEditingController();
  final _keyController = TextEditingController();
  bool _initialized = false;

  @override
  void dispose() {
    _urlController.dispose();
    _keyController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final settings = ref.watch(connectionSettingsProvider);

    return Card(
      margin: const EdgeInsets.all(AppSpacing.md),
      child: Padding(
        padding: const EdgeInsets.all(AppSpacing.md),
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
                const SizedBox(height: AppSpacing.sm),
                TextField(
                  controller: _urlController,
                  decoration: InputDecoration(
                    labelText: l10n.apiUrlLabel,
                    hintText: l10n.defaultHint('http://127.0.0.1:8085'),
                    prefixIcon: const Icon(Icons.dns_outlined),
                  ),
                ),
                const SizedBox(height: AppSpacing.sm),
                TextField(
                  controller: _keyController,
                  obscureText: true,
                  decoration: InputDecoration(
                    labelText: l10n.apiKeyLabelShort,
                    hintText: l10n.apiKeyHint,
                    prefixIcon: const Icon(Icons.key_outlined),
                  ),
                ),
                const SizedBox(height: AppSpacing.sm),
                Align(
                  alignment: Alignment.centerRight,
                  child: FilledButton.tonal(
                    onPressed: () => ref
                        .read(connectionSettingsProvider.notifier)
                        .save(
                          baseUrl: _urlController.text,
                          apiKey: _keyController.text,
                        ),
                    child: Text(l10n.apply),
                  ),
                ),
              ],
            );
          },
        ),
      ),
    );
  }
}
