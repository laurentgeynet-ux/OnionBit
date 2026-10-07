// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../providers/settings_providers.dart';
import 'settings_section.dart';

/// Clé publique de l'identité courante (`GET /api/identity`) —
/// `null` si IPv8 désactivé.
final identityPublicKeyProvider = FutureProvider.autoDispose<String?>(
  (ref) => ref.watch(settingsRepositoryProvider).identityPublicKey(),
);

/// Section « Identité » — export/import portable de la clé secrète
/// IPv8 (blob `OBID` argon2id + ChaCha20-Poly1305 si mot de passe,
/// hex brut sinon). L'import écrase `ipv8_keypair.bin` : la nouvelle
/// identité est active au prochain démarrage du daemon.
class IdentitySection extends ConsumerWidget {
  const IdentitySection({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final pkAsync = ref.watch(identityPublicKeyProvider);
    return SettingsSection(
      icon: Icons.fingerprint,
      title: l10n.sectionIdentity,
      sectionId: 'identity',
      child: (context, settings) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          pkAsync.when(
            loading: () => const LinearProgressIndicator(),
            error: (_, _) => Text(l10n.identityUnavailable),
            data: (pk) => pk == null
                ? Text(l10n.identityUnavailable)
                : Row(
                    children: [
                      Expanded(
                        child: SelectableText(
                          pk,
                          style: const TextStyle(
                            fontFamily: 'monospace',
                            fontSize: 12,
                          ),
                        ),
                      ),
                      IconButton(
                        icon: const Icon(Icons.copy, size: 18),
                        tooltip: l10n.identityCopyKey,
                        onPressed: () async {
                          await Clipboard.setData(ClipboardData(text: pk));
                          if (context.mounted) {
                            ScaffoldMessenger.of(context).showSnackBar(
                              SnackBar(content: Text(l10n.identityCopied)),
                            );
                          }
                        },
                      ),
                    ],
                  ),
          ),
          const SizedBox(height: AppSpacing.sm),
          Text(
            l10n.identityExplain,
            style: Theme.of(context).textTheme.bodySmall?.copyWith(
              color: Theme.of(context).colorScheme.outline,
            ),
          ),
          const SizedBox(height: AppSpacing.sm),
          Wrap(
            spacing: AppSpacing.sm,
            children: [
              FilledButton.tonalIcon(
                icon: const Icon(Icons.upload_outlined, size: 18),
                label: Text(l10n.identityExport),
                onPressed: () => _export(context, ref),
              ),
              FilledButton.tonalIcon(
                icon: const Icon(Icons.download_outlined, size: 18),
                label: Text(l10n.identityImport),
                onPressed: () => _import(context, ref),
              ),
            ],
          ),
        ],
      ),
    );
  }

  /// Exporte la clé : dialogue mot de passe optionnel (blob `OBID`
  /// chiffré si renseigné) → copie hex dans le presse-papiers.
  Future<void> _export(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final pw = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityExport),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(l10n.identityExportExplain),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: pw,
              obscureText: true,
              decoration: InputDecoration(
                labelText: l10n.identityPasswordOptional,
                border: const OutlineInputBorder(),
              ),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.identityExport),
          ),
        ],
      ),
    );
    if (ok != true || !context.mounted) return;
    try {
      final resp = await ref
          .read(settingsRepositoryProvider)
          .identityExport(password: pw.text.isEmpty ? null : pw.text);
      final key = resp['key'] as String? ?? '';
      await Clipboard.setData(ClipboardData(text: key));
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(l10n.identityExported)),
        );
      }
    } catch (e) {
      if (context.mounted) _error(context, e);
    } finally {
      pw.dispose();
    }
  }

  /// Importe une clé (hex brut ou blob `OBID`) — écrase
  /// `ipv8_keypair.bin` après validation côté daemon, puis invite
  /// au redémarrage.
  Future<void> _import(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final key = TextEditingController();
    final pw = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityImport),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(l10n.identityImportExplain),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: key,
              style: const TextStyle(fontFamily: 'monospace', fontSize: 11),
              maxLines: 4,
              decoration: InputDecoration(
                labelText: l10n.identityKeyHex,
                border: const OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: pw,
              obscureText: true,
              decoration: InputDecoration(
                labelText: l10n.identityPasswordOptional,
                border: const OutlineInputBorder(),
              ),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.identityImport),
          ),
        ],
      ),
    );
    final keyHex = key.text.trim();
    final password = pw.text;
    key.dispose();
    pw.dispose();
    if (ok != true || !context.mounted || keyHex.isEmpty) return;
    try {
      await ref
          .read(settingsRepositoryProvider)
          .identityRestore(keyHex, password: password.isEmpty ? null : password);
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(l10n.identityRestoredRestart)),
        );
      }
    } catch (e) {
      if (context.mounted) _error(context, e);
    }
  }

  void _error(BuildContext context, Object e) {
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text('$e')));
  }
}
