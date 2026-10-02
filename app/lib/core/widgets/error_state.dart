// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../api/api_client.dart';
import '../l10n/l10n_ext.dart';
import '../theme/app_theme.dart';

/// État d'erreur générique avec action de nouvelle tentative — consommé
/// par les écrans branchés sur un `AsyncValue.error` Riverpod.
///
/// Un [DaemonUnreachableException] produit un message orienté cause
/// (« le daemon ne répond pas ») plutôt que la trace HTTP brute ; les
/// [ApiException] affichent leur `message` métier sans le préfixe
/// technique ; tout le reste retombe sur `toString()`.
class ErrorState extends StatelessWidget {
  const ErrorState({super.key, required this.error, this.onRetry});

  final Object error;
  final VoidCallback? onRetry;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final e = error;
    final unreachable = e is DaemonUnreachableException;
    final icon = unreachable ? Icons.cloud_off_outlined : Icons.error_outline;
    final message = switch (e) {
      DaemonUnreachableException(:final uri) => context.l10n
          .daemonUnreachableBody('$uri'),
      ApiException(:final message) => message,
      _ => '$e',
    };
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(AppSpacing.lg),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 48, color: theme.colorScheme.error),
            const SizedBox(height: AppSpacing.md),
            Text(
              unreachable
                  ? context.l10n.daemonUnreachableTitle
                  : message,
              style: theme.textTheme.titleMedium,
              textAlign: TextAlign.center,
            ),
            if (unreachable) ...[
              const SizedBox(height: AppSpacing.sm),
              Text(
                message,
                style: theme.textTheme.bodyMedium,
                textAlign: TextAlign.center,
              ),
            ],
            if (onRetry != null) ...[
              const SizedBox(height: AppSpacing.md),
              FilledButton.tonal(
                onPressed: onRetry,
                child: Text(context.l10n.retry),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
