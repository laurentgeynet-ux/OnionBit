// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../providers/settings_providers.dart';
import 'settings_section.dart';

/// Statut complet de l'identité (`GET /api/identity` — ADR-0016) :
/// `{state: ready|locked|pending, seeded, mode, persistent,
/// public_key?}`.
final identityStatusProvider =
    FutureProvider.autoDispose<Map<String, dynamic>>(
      (ref) => ref.watch(settingsRepositoryProvider).identityStatus(),
    );

/// Rappel « phrase non confirmée » masqué pour la session courante —
/// il réapparaît au prochain lancement tant que
/// `identity.seed_acknowledged` n'est pas vrai côté daemon (décision
/// ADR-0016 : insistant, non bloquant).
final seedReminderDismissedProvider =
    NotifierProvider<_SeedReminderDismissed, bool>(
      _SeedReminderDismissed.new,
    );

class _SeedReminderDismissed extends Notifier<bool> {
  @override
  bool build() => false;

  void dismiss() => state = true;
}

/// Section « Identité » (ADR-0016) — identité seedée (phrase BIP39
/// 24 mots EN/FR), export/import `OBID` legacy, chiffrement at-rest
/// `OBSK`, sessions invitées. Le gate `pending`/`locked` est traité
/// par les écrans de démarrage ; ici on reflète l'état.
class IdentitySection extends ConsumerWidget {
  const IdentitySection({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final statusAsync = ref.watch(identityStatusProvider);
    return SettingsSection(
      icon: Icons.fingerprint,
      title: l10n.sectionIdentity,
      sectionId: 'identity',
      child: (context, settings) => statusAsync.when(
        loading: () => const LinearProgressIndicator(),
        error: (_, _) => Text(l10n.identityUnavailable),
        data: (status) => _body(context, ref, settings, status),
      ),
    );
  }

  Widget _body(
    BuildContext context,
    WidgetRef ref,
    Map<String, dynamic> settings,
    Map<String, dynamic> status,
  ) {
    final l10n = context.l10n;
    final state = status['state'] as String? ?? 'ready';
    if (state != 'ready') {
      return Text(
        state == 'locked' ? l10n.identityStateLocked : l10n.identityStatePending,
      );
    }
    final pk = status['public_key'] as String?;
    final seeded = status['seeded'] == true;
    final guest = status['mode'] == 'guest';
    final acknowledged = settingsBool(settings, const [
      'identity',
      'seed_acknowledged',
    ]);
    final dismissed = ref.watch(seedReminderDismissedProvider);
    final atRest = settingsBool(settings, const ['identity', 'at_rest']);
    // `at_rest` refuse stealth.role bridge/gateway (fail-closed) —
    // le switch est desactive dans ce cas plutot qu'un 400 tardif.
    final stealthBlocksAtRest =
        settingsBool(settings, const ['stealth', 'enabled']) &&
        settingsString(settings, const ['stealth', 'role']) != 'client';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (pk == null)
          Text(l10n.identityUnavailable)
        else
          Row(
            children: [
              Expanded(
                child: SelectableText(
                  pk,
                  style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
                ),
              ),
              IconButton(
                icon: const Icon(Icons.copy, size: 18),
                tooltip: l10n.identityCopyKey,
                onPressed: () => _copy(context, pk, l10n.identityCopied),
              ),
            ],
          ),
        const SizedBox(height: AppSpacing.sm),
        Text(
          l10n.identityExplain,
          style: Theme.of(
            context,
          ).textTheme.bodySmall?.copyWith(
            color: Theme.of(context).colorScheme.outline,
          ),
        ),
        if (guest) ...[
          const SizedBox(height: AppSpacing.sm),
          Text(
            l10n.identityGuestNote,
            style: Theme.of(context).textTheme.bodySmall?.copyWith(
              color: Theme.of(context).colorScheme.tertiary,
            ),
          ),
        ],
        // Rappel « noter la phrase » : affiche tant que l'utilisateur
        // n'a pas confirme — dismissible pour la session seulement.
        if (seeded && !guest && !acknowledged && !dismissed) ...[
          const SizedBox(height: AppSpacing.sm),
          Card(
            color: Theme.of(context).colorScheme.errorContainer,
            child: Padding(
              padding: const EdgeInsets.all(AppSpacing.sm),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    l10n.identityReminderTitle,
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
                  const SizedBox(height: AppSpacing.xs),
                  Text(
                    l10n.identityReminderBody,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                  const SizedBox(height: AppSpacing.sm),
                  Wrap(
                    spacing: AppSpacing.sm,
                    children: [
                      FilledButton.tonalIcon(
                        icon: const Icon(Icons.key, size: 18),
                        label: Text(l10n.identityPhraseShow),
                        onPressed: () => _showPhrase(context, ref),
                      ),
                      FilledButton(
                        onPressed: () => _acknowledge(context, ref),
                        child: Text(l10n.identityReminderConfirm),
                      ),
                      TextButton(
                        onPressed: () => ref
                            .read(seedReminderDismissedProvider.notifier)
                            .dismiss(),
                        child: Text(l10n.identityReminderLater),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ],
        const SizedBox(height: AppSpacing.sm),
        Wrap(
          spacing: AppSpacing.sm,
          runSpacing: AppSpacing.sm,
          children: [
            if (seeded)
              FilledButton.tonalIcon(
                icon: const Icon(Icons.key, size: 18),
                label: Text(l10n.identityPhraseShow),
                onPressed: () => _showPhrase(context, ref),
              ),
            FilledButton.tonalIcon(
              icon: const Icon(Icons.upload_outlined, size: 18),
              label: Text(l10n.identityExport),
              onPressed: () => _export(context, ref),
            ),
            FilledButton.tonalIcon(
              icon: const Icon(Icons.download_outlined, size: 18),
              label: Text(l10n.identityImport),
              onPressed: () => _import(context, ref, seeded),
            ),
          ],
        ),
        if (seeded && !guest) ...[
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: Text(l10n.identityAtRest),
            subtitle: Text(
              stealthBlocksAtRest
                  ? l10n.identityAtRestBlocked
                  : l10n.identityAtRestSub,
            ),
            value: atRest,
            onChanged: stealthBlocksAtRest
                ? null
                : (v) => _setAtRest(context, ref, v),
          ),
        ],
      ],
    );
  }

  Future<void> _copy(BuildContext context, String text, String snack) async {
    await Clipboard.setData(ClipboardData(text: text));
    if (context.mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(snack)));
    }
  }

  /// Phrase BIP39 : chargée à l'ouverture du dialogue (langue = locale
  /// de l'UI), 24 mots numérotés en grille, copie en un geste.
  Future<void> _showPhrase(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final lang = Localizations.localeOf(context).languageCode;
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityPhraseTitle),
        content: FutureBuilder<String?>(
          future: ref
              .read(settingsRepositoryProvider)
              .identityRecoveryPhrase(lang: lang),
          builder: (ctx, snap) {
            if (!snap.hasData) {
              return const SizedBox(
                width: 320,
                height: 80,
                child: Center(child: CircularProgressIndicator()),
              );
            }
            final phrase = snap.data;
            if (phrase == null) {
              return Text(l10n.identityPhraseUnavailable);
            }
            final words = phrase.split(' ');
            return SizedBox(
              width: 420,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(l10n.identityPhraseExplain),
                  const SizedBox(height: AppSpacing.sm),
                  Wrap(
                    spacing: AppSpacing.xs,
                    runSpacing: AppSpacing.xs,
                    children: [
                      for (var i = 0; i < words.length; i++)
                        Chip(
                          visualDensity: VisualDensity.compact,
                          label: Text(
                            '${i + 1}. ${words[i]}',
                            style: const TextStyle(
                              fontFamily: 'monospace',
                              fontSize: 12,
                            ),
                          ),
                        ),
                    ],
                  ),
                ],
              ),
            );
          },
        ),
        actions: [
          TextButton.icon(
            icon: const Icon(Icons.copy, size: 18),
            label: Text(l10n.identityPhraseCopy),
            onPressed: () async {
              final phrase = await ref
                  .read(settingsRepositoryProvider)
                  .identityRecoveryPhrase(lang: lang);
              if (phrase != null && ctx.mounted) {
                await Clipboard.setData(ClipboardData(text: phrase));
                if (ctx.mounted) {
                  ScaffoldMessenger.of(ctx).showSnackBar(
                    SnackBar(content: Text(l10n.identityPhraseCopied)),
                  );
                }
              }
            },
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx),
            child: Text(l10n.close),
          ),
        ],
      ),
    );
  }

  /// « J'ai noté ma phrase » : persiste `identity.seed_acknowledged`
  /// côté daemon (survit au redémarrage et entre clients), puis
  /// propose le chiffrement at-rest (décision ADR-0016 : proposé à
  /// la confirmation de la phrase).
  Future<void> _acknowledge(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityReminderConfirm),
        content: Text(l10n.identityPhraseExplain),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.identityReminderConfirm),
          ),
        ],
      ),
    );
    if (ok != true || !context.mounted) return;
    try {
      await ref.read(settingsRepositoryProvider).update({
        'identity': {'seed_acknowledged': true},
      });
      ref.invalidate(daemonSettingsProvider);
    } catch (e) {
      if (context.mounted) _error(context, e);
      return;
    }
    if (context.mounted) _offerAtRest(context, ref);
  }

  /// Proposition at-rest après confirmation de la phrase.
  Future<void> _offerAtRest(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final yes = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityAtRestOfferTitle),
        content: Text(l10n.identityAtRestOfferBody),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.identityAtRestOfferSkip),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.identityAtRestOfferYes),
          ),
        ],
      ),
    );
    if (yes == true && context.mounted) {
      _setAtRest(context, ref, true);
    }
  }

  /// Bascule `identity.at_rest` : mot de passe exigé dans les deux
  /// sens (nouveau pour sceller, courant pour désceller).
  Future<void> _setAtRest(
    BuildContext context,
    WidgetRef ref,
    bool enabled,
  ) async {
    final l10n = context.l10n;
    final pw = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityAtRest),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(l10n.identityAtRestSub),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: pw,
              obscureText: true,
              autofocus: true,
              decoration: InputDecoration(
                labelText: enabled
                    ? l10n.identityAtRestPasswordNew
                    : l10n.identityAtRestPasswordCurrent,
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
            child: Text(l10n.apply),
          ),
        ],
      ),
    );
    final password = pw.text;
    pw.dispose();
    if (ok != true || !context.mounted || password.isEmpty) return;
    try {
      await ref
          .read(settingsRepositoryProvider)
          .identitySetAtRest(enabled: enabled, password: password);
      ref.invalidate(daemonSettingsProvider);
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(l10n.identityAtRestDone)),
        );
      }
    } catch (e) {
      if (context.mounted) _error(context, e);
    }
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

  /// Importe une phrase BIP39 (24 mots, EN/FR) **ou** une clé
  /// `LibNaCLSK:`/blob `OBID` — détection automatique : l'hex pur
  /// va au chemin clé, tout le reste au chemin phrase. Sur install
  /// seedée, la clé brute exige `force_legacy` (confirmation préalable
  /// — la graine gagnerait sinon au prochain boot).
  Future<void> _import(
    BuildContext context,
    WidgetRef ref,
    bool seeded,
  ) async {
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
                labelText: l10n.identityKeyOrPhrase,
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
    final input = key.text.trim();
    final password = pw.text;
    key.dispose();
    pw.dispose();
    if (ok != true || !context.mounted || input.isEmpty) return;
    try {
      // Hex pur (sans espaces) = cle/blob ; le reste = phrase BIP39.
      final compact = input.replaceAll(RegExp(r'\s'), '');
      if (RegExp(r'^[0-9a-fA-F]+$').hasMatch(compact) &&
          compact.length >= 64) {
        var forceLegacy = false;
        if (seeded) {
          forceLegacy = await _confirmForceLegacy(context) ?? false;
          if (!forceLegacy || !context.mounted) return;
        }
        await ref
            .read(settingsRepositoryProvider)
            .identityRestore(
              compact,
              password: password.isEmpty ? null : password,
              forceLegacy: forceLegacy,
            );
      } else {
        await ref
            .read(settingsRepositoryProvider)
            .identityRestorePhrase(input);
      }
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(l10n.identityRestoredRestart)),
        );
      }
    } catch (e) {
      if (context.mounted) _error(context, e);
    }
  }

  /// Confirmation « seedée → legacy » avant un import par clé brute.
  Future<bool?> _confirmForceLegacy(BuildContext context) {
    final l10n = context.l10n;
    return showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityForceLegacyTitle),
        content: Text(l10n.identityForceLegacyBody),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: Text(l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(l10n.identityForceLegacyConfirm),
          ),
        ],
      ),
    );
  }

  void _error(BuildContext context, Object e) {
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text('$e')));
  }
}
