// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Assistant de premier démarrage (ADR-0021 §8, étape 76) — remplace
/// le gate `identity_pending` en liste plate par un parcours en
/// étapes : choix → création (mot de passe at-rest optionnel) ou
/// restauration → sauvegarde de la phrase de récupération.
///
/// La phrase est remise à [pendingRecoveryPhraseProvider] et rendue en
/// **overlay global** par `app.dart` — pendant `pending`, le `builder`
/// de `MaterialApp` remplace tout le contenu routé : aucun `Navigator`
/// n'existe pour `showDialog` (un `Navigator.of` y levait une
/// exception silencieuse et « Create my identity » semblait inerte).
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../features/settings/presentation/providers/settings_providers.dart';
import '../design/design_tokens.dart';
import '../l10n/l10n_ext.dart';

/// Phrase BIP39 en attente de présentation après `identity/create`.
/// Posée par le wizard, consommée par l'overlay de `app.dart` qui
/// affiche [PhraseBackupDialog] au-dessus du gate comme de la
/// coquille — la session est déjà `ready`, l'utilisateur doit
/// confirmer la sauvegarde avant de continuer.
final pendingRecoveryPhraseProvider =
    NotifierProvider<PendingRecoveryPhraseNotifier, String?>(
      PendingRecoveryPhraseNotifier.new,
    );

class PendingRecoveryPhraseNotifier extends Notifier<String?> {
  @override
  String? build() => null;

  void present(String phrase) => state = phrase;
  void clear() => state = null;
}

/// Premier boot — parcours en étapes des trois résolutions.
class OnboardingWizard extends ConsumerStatefulWidget {
  const OnboardingWizard({super.key});

  @override
  ConsumerState<OnboardingWizard> createState() => _OnboardingWizardState();
}

enum _Step { choice, create, restore }

class _OnboardingWizardState extends ConsumerState<OnboardingWizard> {
  _Step _step = _Step.choice;
  bool _busy = false;
  String? _error;

  Future<void> _run(Future<void> Function() action) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action();
      // La résolution fait passer la session en `ready` — le provider
      // rafraîchit et l'écran disparaît de lui-même.
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Création → sauvegarde de la phrase : remise à
  /// [pendingRecoveryPhraseProvider] après la résolution — l'overlay
  /// de `app.dart` l'affiche au-dessus du gate/coquille, sans dépendre
  /// d'un `Navigator` (absent de l'arbre tant que `pending`).
  Future<void> _create(String password) async {
    String? phrase;
    await _run(() async {
      final repo = ref.read(settingsRepositoryProvider);
      await repo.identityCreate(
        password: password.isEmpty ? null : password,
      );
      phrase = await repo.identityRecoveryPhrase();
    });
    final p = phrase;
    if (p != null && p.isNotEmpty && mounted) {
      ref.read(pendingRecoveryPhraseProvider.notifier).present(p);
    }
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 250),
      child: switch (_step) {
        _Step.choice => _ChoiceStep(
          busy: _busy,
          error: _error,
          onCreate: () => setState(() => _step = _Step.create),
          onRestore: () => setState(() => _step = _Step.restore),
          onGuest: _busy
              ? null
              : () => _run(
                  () => ref
                      .read(settingsRepositoryProvider)
                      .identityGuest(),
                ),
        ),
        _Step.create => _CreateStep(
          busy: _busy,
          error: _error,
          onBack: () => setState(() => _step = _Step.choice),
          onCreate: _create,
        ),
        _Step.restore => _RestoreStep(
          busy: _busy,
          error: _error,
          onBack: () => setState(() => _step = _Step.choice),
          onRestore: (input, password) => _run(() async {
            final repo = ref.read(settingsRepositoryProvider);
            // Hex pur = clé/blob `OBID` ; le reste = phrase BIP39.
            final compact = input.replaceAll(RegExp(r'\s'), '');
            if (RegExp(r'^[0-9a-fA-F]+$').hasMatch(compact) &&
                compact.length >= 64) {
              await repo.identityRestore(
                compact,
                password: password.isEmpty ? null : password,
              );
            } else {
              await repo.identityRestorePhrase(input);
            }
          }),
        ),
      },
    );
  }
}

/// En-tête de step avec retour au choix.
Widget _stepHeader(
  BuildContext context,
  VoidCallback onBack, {
  required IconData icon,
  required String title,
}) {
  final theme = Theme.of(context);
  return Column(
    children: [
      Align(
        alignment: Alignment.centerLeft,
        child: IconButton(
          onPressed: onBack,
          icon: const Icon(Icons.arrow_back, size: 20),
          tooltip: MaterialLocalizations.of(context).backButtonTooltip,
        ),
      ),
      Icon(icon, size: 56, color: theme.colorScheme.primary),
      const SizedBox(height: AppSpace.sm),
      Text(
        title,
        style: theme.textTheme.headlineSmall,
        textAlign: TextAlign.center,
      ),
    ],
  );
}

Widget _busyError(bool busy, String? error, BuildContext context) =>
    Column(
      children: [
        if (busy) ...[
          const SizedBox(height: AppSpace.md),
          const Center(child: CircularProgressIndicator()),
        ],
        if (error != null) ...[
          const SizedBox(height: AppSpace.md),
          Text(
            error,
            style: TextStyle(color: Theme.of(context).colorScheme.error),
            textAlign: TextAlign.center,
          ),
        ],
      ],
    );

/// Étape 1 — les trois résolutions du gate en cartes premium.
class _ChoiceStep extends StatelessWidget {
  const _ChoiceStep({
    required this.busy,
    required this.error,
    required this.onCreate,
    required this.onRestore,
    required this.onGuest,
  });

  final bool busy;
  final String? error;
  final VoidCallback onCreate;
  final VoidCallback onRestore;
  final VoidCallback? onGuest;

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Icon(
          Icons.fingerprint,
          size: 64,
          color: Theme.of(context).colorScheme.primary,
        ),
        const SizedBox(height: AppSpace.md),
        Text(
          l10n.gatePendingTitle,
          style: Theme.of(context).textTheme.headlineSmall,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpace.sm),
        Text(
          l10n.gatePendingBody,
          style: Theme.of(context).textTheme.bodyMedium,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpace.lg),
        _GateCard(
          icon: Icons.auto_awesome,
          title: l10n.gateNewIdentity,
          subtitle: l10n.gateNewIdentitySub,
          onTap: busy ? null : onCreate,
        ),
        const SizedBox(height: AppSpace.sm),
        _GateCard(
          icon: Icons.restore,
          title: l10n.gateRestore,
          subtitle: l10n.gateRestoreSub,
          onTap: busy ? null : onRestore,
        ),
        const SizedBox(height: AppSpace.sm),
        _GateCard(
          icon: Icons.person_off_outlined,
          title: l10n.gateGuest,
          subtitle: l10n.gateGuestSub,
          onTap: onGuest,
        ),
        _busyError(busy, error, context),
      ],
    );
  }
}

/// Étape « nouvelle identité » — mot de passe at-rest optionnel puis
/// création (la phrase est présentée par `_PhraseBackupDialog`).
class _CreateStep extends StatefulWidget {
  const _CreateStep({
    required this.busy,
    required this.error,
    required this.onBack,
    required this.onCreate,
  });

  final bool busy;
  final String? error;
  final VoidCallback onBack;
  final Future<void> Function(String password) onCreate;

  @override
  State<_CreateStep> createState() => _CreateStepState();
}

class _CreateStepState extends State<_CreateStep> {
  final _password = TextEditingController();

  @override
  void dispose() {
    _password.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _stepHeader(
          context,
          widget.onBack,
          icon: Icons.auto_awesome,
          title: l10n.gateNewIdentity,
        ),
        const SizedBox(height: AppSpace.sm),
        Text(
          l10n.onboardCreateBody,
          style: Theme.of(context).textTheme.bodyMedium,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpace.lg),
        TextField(
          controller: _password,
          enabled: !widget.busy,
          obscureText: true,
          decoration: InputDecoration(
            labelText: l10n.identityAtRestPasswordNew,
            helperText: l10n.onboardCreatePasswordHint,
            prefixIcon: const Icon(Icons.lock_outline, size: 18),
            border: const OutlineInputBorder(),
          ),
          onSubmitted: (_) => widget.onCreate(_password.text),
        ),
        const SizedBox(height: AppSpace.md),
        FilledButton.icon(
          onPressed: widget.busy
              ? null
              : () => widget.onCreate(_password.text),
          icon: const Icon(Icons.fingerprint, size: 18),
          label: Text(l10n.onboardCreateGo),
        ),
        _busyError(widget.busy, widget.error, context),
      ],
    );
  }
}

/// Étape « restaurer » — phrase BIP39 ou clé/blob `OBID` (hex) +
/// mot de passe d'export éventuel.
class _RestoreStep extends StatefulWidget {
  const _RestoreStep({
    required this.busy,
    required this.error,
    required this.onBack,
    required this.onRestore,
  });

  final bool busy;
  final String? error;
  final VoidCallback onBack;
  final Future<void> Function(String input, String password) onRestore;

  @override
  State<_RestoreStep> createState() => _RestoreStepState();
}

class _RestoreStepState extends State<_RestoreStep> {
  final _input = TextEditingController();
  final _password = TextEditingController();

  @override
  void dispose() {
    _input.dispose();
    _password.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _stepHeader(
          context,
          widget.onBack,
          icon: Icons.restore,
          title: l10n.gateRestore,
        ),
        const SizedBox(height: AppSpace.sm),
        Text(
          l10n.gateRestoreSub,
          style: Theme.of(context).textTheme.bodyMedium,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpace.lg),
        TextField(
          controller: _input,
          enabled: !widget.busy,
          maxLines: 3,
          style: AppTypography.mono(fontSize: 13),
          decoration: InputDecoration(
            labelText: l10n.identityKeyOrPhrase,
            border: const OutlineInputBorder(),
          ),
        ),
        const SizedBox(height: AppSpace.sm),
        TextField(
          controller: _password,
          enabled: !widget.busy,
          obscureText: true,
          decoration: InputDecoration(
            labelText: l10n.identityPasswordOptional,
            border: const OutlineInputBorder(),
          ),
          onSubmitted: (_) => widget.onRestore(
            _input.text.trim(),
            _password.text,
          ),
        ),
        const SizedBox(height: AppSpace.md),
        FilledButton.icon(
          onPressed: widget.busy
              ? null
              : () => widget.onRestore(
                  _input.text.trim(),
                  _password.text,
                ),
          icon: const Icon(Icons.download_outlined, size: 18),
          label: Text(l10n.gateRestoreGo),
        ),
        _busyError(widget.busy, widget.error, context),
      ],
    );
  }
}

/// Sauvegarde de la phrase de récupération — dialogue modal
/// obligatoire rendu en overlay par `app.dart` après `create` (il
/// survit à la résolution du gate : la coquille apparaît dessous).
/// `onDone` referme l'overlay — ce widget n'est pas une route, il ne
/// se pop pas lui-même.
class PhraseBackupDialog extends StatefulWidget {
  const PhraseBackupDialog({
    super.key,
    required this.phrase,
    required this.onDone,
  });

  final String phrase;
  final VoidCallback onDone;

  @override
  State<PhraseBackupDialog> createState() => _PhraseBackupDialogState();
}

class _PhraseBackupDialogState extends State<PhraseBackupDialog> {
  bool _noted = false;

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final theme = Theme.of(context);
    final words = widget.phrase.trim().split(RegExp(r'\s+'));
    return AlertDialog(
      title: Text(l10n.identityPhraseTitle),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              l10n.identityPhraseExplain,
              style: theme.textTheme.bodyMedium,
            ),
            const SizedBox(height: AppSpace.md),
            // Grille de mots numérotés en JetBrains Mono — le format
            // de présentation des phrases BIP39 le moins ambigu.
            Wrap(
              spacing: AppSpace.xs,
              runSpacing: AppSpace.xs,
              children: [
                for (var i = 0; i < words.length; i++)
                  Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: AppSpace.sm,
                      vertical: 2,
                    ),
                    decoration: BoxDecoration(
                      color: theme.colorScheme.surfaceContainerHigh,
                      borderRadius: BorderRadius.circular(
                        AppRadius.small,
                      ),
                    ),
                    child: Text(
                      '${i + 1}. ${words[i]}',
                      style: AppTypography.mono(fontSize: 12),
                    ),
                  ),
              ],
            ),
            const SizedBox(height: AppSpace.md),
            OutlinedButton.icon(
              onPressed: () async {
                await Clipboard.setData(
                  ClipboardData(text: widget.phrase),
                );
                if (context.mounted) {
                  ScaffoldMessenger.of(context).showSnackBar(
                    SnackBar(content: Text(l10n.identityPhraseCopied)),
                  );
                }
              },
              icon: const Icon(Icons.copy, size: 16),
              label: Text(l10n.identityPhraseCopy),
            ),
            CheckboxListTile(
              value: _noted,
              onChanged: (v) => setState(() => _noted = v ?? false),
              title: Text(l10n.identityReminderConfirm),
              controlAffinity: ListTileControlAffinity.leading,
              contentPadding: EdgeInsets.zero,
            ),
          ],
        ),
      ),
      actions: [
        FilledButton(
          onPressed: _noted ? widget.onDone : null,
          child: Text(l10n.onboardDone),
        ),
      ],
    );
  }
}

/// Carte de choix cliquable (étape « choice »).
class _GateCard extends StatelessWidget {
  const _GateCard({
    required this.icon,
    required this.title,
    required this.subtitle,
    this.onTap,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        leading: Icon(icon),
        title: Text(title),
        subtitle: Text(subtitle),
        onTap: onTap,
      ),
    );
  }
}
