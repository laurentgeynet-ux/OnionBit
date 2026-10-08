// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Gate identitaire ADR-0016 (étape 48e) : tant que le daemon est en
/// `identity_pending` (premier boot sous `--first-run-gate`) ou
/// `locked` (graine `OBSK`), l'UI remplace tout le contenu routé par
/// l'écran de résolution — aucune requête identitaire n'a de sens
/// tant que l'identité n'existe pas (l'API répondrait `409`).
///
/// - pending : trois résolutions — nouvelle identité, restauration
///   (phrase BIP39 ou clé `OBID`), session invitée éphémère ;
/// - locked : mot de passe `OBSK` → `POST /api/identity/unlock`
///   (400 = mot de passe incorrect, 429 = rate-limit) ; la session
///   invitée reste une porte de sortie assumée ;
/// - `mode == "guest"` en `ready` : bandeau persistant « rien n'est
///   conservé ».
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../features/settings/presentation/providers/settings_providers.dart';
import '../api/api_client.dart';
import '../api/events.dart';
import '../api/sse_client.dart';
import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import '../theme/app_theme.dart';

/// Statut identitaire du daemon (ADR-0016). Pas de polling (timers
/// interdits en test) : réévalué sur `events_start` (session résolue
/// après unlock/create/guest/restore) et sur chaque transition de
/// connexion SSE — un redémarrage en `locked` est ainsi détecté à la
/// reconnexion. Erreur/daemon injoignable → `value == null` (la
/// bannière « daemon injoignable » existante s'en charge).
final identityGateProvider =
    FutureProvider.autoDispose<Map<String, dynamic>>((ref) {
      ref.listen<AsyncValue<SseEvent>>(daemonEventsProvider, (_, next) {
        if (next.value?.topic == EventTopics.eventsStart) {
          ref.invalidateSelf();
        }
      });
      ref.listen<AsyncValue<bool>>(sseConnectedProvider, (prev, next) {
        if (prev?.value != next.value) ref.invalidateSelf();
      });
      return ref.watch(settingsRepositoryProvider).identityStatus();
    });

/// Écran pleine page du gate — rendu à la place du contenu routé
/// depuis le `builder` de `MaterialApp` quand `state != "ready"`.
class IdentityGatePage extends StatelessWidget {
  const IdentityGatePage({super.key, required this.locked});

  /// `true` = graine `OBSK` scellée (unlock) ; `false` = premier boot.
  final bool locked;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 520),
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(AppSpacing.lg),
            child: locked
                ? const _LockedGate()
                : const _PendingGate(),
          ),
        ),
      ),
    );
  }
}

/// Bandeau permanent « session invitée » (rien n'est conservé).
class GuestBanner extends StatelessWidget {
  const GuestBanner({super.key});

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Material(
      color: scheme.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: AppSpacing.md,
          vertical: AppSpacing.xs,
        ),
        child: Row(
          children: [
            Icon(Icons.person_off_outlined, size: 18, color: scheme.onTertiaryContainer),
            const SizedBox(width: AppSpacing.sm),
            Expanded(
              child: Text(
                context.l10n.guestBanner,
                style: TextStyle(color: scheme.onTertiaryContainer),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Bandeau « média amovible » (ADR-0018 étape 63) : quand la racine
/// du bundle vit sur un volume amovible ou sans ACL (`storage_
/// removable` de `GET /api/identity`), on propose de sceller la
/// graine `identity.at_rest` — proposition non bloquante, masquable
/// pour la session. Jamais affiché en session invitée ni si la
/// graine est déjà scellée.
class RemovableStorageBanner extends ConsumerStatefulWidget {
  const RemovableStorageBanner({super.key});

  @override
  ConsumerState<RemovableStorageBanner> createState() =>
      _RemovableStorageBannerState();
}

class _RemovableStorageBannerState
    extends ConsumerState<RemovableStorageBanner> {
  bool _dismissed = false;
  bool _busy = false;

  /// `identity.at_rest` exige un mot de passe (scellement `OBSK`).
  Future<void> _enableAtRest() async {
    final l10n = context.l10n;
    final pw = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(l10n.identityAtRest),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(l10n.removableBannerBody),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: pw,
              obscureText: true,
              autofocus: true,
              decoration: InputDecoration(
                labelText: l10n.identityAtRestPasswordNew,
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
    if (ok != true || !mounted || password.isEmpty) return;
    setState(() => _busy = true);
    try {
      await ref
          .read(settingsRepositoryProvider)
          .identitySetAtRest(enabled: true, password: password);
      ref.invalidate(daemonSettingsProvider);
      ref.invalidate(identityGateProvider);
      if (mounted) setState(() => _dismissed = true);
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('$e')));
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_dismissed) return const SizedBox.shrink();
    final scheme = Theme.of(context).colorScheme;
    final l10n = context.l10n;
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
              Icons.usb_outlined,
              size: 18,
              color: scheme.onErrorContainer,
            ),
            const SizedBox(width: AppSpacing.sm),
            Expanded(
              child: Text(
                l10n.removableBannerBody,
                style: TextStyle(color: scheme.onErrorContainer),
              ),
            ),
            TextButton(
              onPressed: () => setState(() => _dismissed = true),
              child: Text(l10n.removableBannerDismiss),
            ),
            FilledButton.tonal(
              onPressed: _busy ? null : _enableAtRest,
              child: Text(l10n.removableBannerAction),
            ),
          ],
        ),
      ),
    );
  }
}

/// Premier boot : trois résolutions du gate `identity_pending`.
class _PendingGate extends ConsumerStatefulWidget {
  const _PendingGate();

  @override
  ConsumerState<_PendingGate> createState() => _PendingGateState();
}

class _PendingGateState extends ConsumerState<_PendingGate> {
  final _restoreField = TextEditingController();
  final _restorePassword = TextEditingController();
  bool _restoreOpen = false;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _restoreField.dispose();
    _restorePassword.dispose();
    super.dispose();
  }

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

  void _restore() {
    final input = _restoreField.text.trim();
    if (input.isEmpty) return;
    final password = _restorePassword.text;
    final repo = ref.read(settingsRepositoryProvider);
    // Hex pur = clé/blob `OBID` ; le reste = phrase BIP39 (EN/FR).
    final compact = input.replaceAll(RegExp(r'\s'), '');
    if (RegExp(r'^[0-9a-fA-F]+$').hasMatch(compact) &&
        compact.length >= 64) {
      _run(
        () => repo.identityRestore(
          compact,
          password: password.isEmpty ? null : password,
        ),
      );
    } else {
      _run(() => repo.identityRestorePhrase(input));
    }
  }

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
        const SizedBox(height: AppSpacing.md),
        Text(
          l10n.gatePendingTitle,
          style: Theme.of(context).textTheme.headlineSmall,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpacing.sm),
        Text(
          l10n.gatePendingBody,
          style: Theme.of(context).textTheme.bodyMedium,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpacing.lg),
        _GateCard(
          icon: Icons.auto_awesome,
          title: l10n.gateNewIdentity,
          subtitle: l10n.gateNewIdentitySub,
          onTap: _busy
              ? null
              : () => _run(
                  () => ref
                      .read(settingsRepositoryProvider)
                      .identityCreate(),
                ),
        ),
        const SizedBox(height: AppSpacing.sm),
        _GateCard(
          icon: Icons.restore,
          title: l10n.gateRestore,
          subtitle: l10n.gateRestoreSub,
          onTap: _busy
              ? null
              : () => setState(() => _restoreOpen = !_restoreOpen),
        ),
        if (_restoreOpen) ...[
          const SizedBox(height: AppSpacing.sm),
          TextField(
            controller: _restoreField,
            enabled: !_busy,
            maxLines: 3,
            style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
            decoration: InputDecoration(
              labelText: l10n.identityKeyOrPhrase,
              border: const OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: AppSpacing.sm),
          TextField(
            controller: _restorePassword,
            enabled: !_busy,
            obscureText: true,
            decoration: InputDecoration(
              labelText: l10n.identityPasswordOptional,
              border: const OutlineInputBorder(),
            ),
            onSubmitted: (_) => _restore(),
          ),
          const SizedBox(height: AppSpacing.sm),
          FilledButton.icon(
            onPressed: _busy ? null : _restore,
            icon: const Icon(Icons.download_outlined, size: 18),
            label: Text(l10n.gateRestoreGo),
          ),
        ],
        const SizedBox(height: AppSpacing.sm),
        _GateCard(
          icon: Icons.person_off_outlined,
          title: l10n.gateGuest,
          subtitle: l10n.gateGuestSub,
          onTap: _busy
              ? null
              : () => _run(
                  () => ref
                      .read(settingsRepositoryProvider)
                      .identityGuest(),
                ),
        ),
        if (_busy) ...[
          const SizedBox(height: AppSpacing.md),
          const Center(child: CircularProgressIndicator()),
        ],
        if (_error != null) ...[
          const SizedBox(height: AppSpacing.md),
          Text(
            _error!,
            style: TextStyle(color: Theme.of(context).colorScheme.error),
            textAlign: TextAlign.center,
          ),
        ],
      ],
    );
  }
}

/// Boot `locked` : mot de passe `OBSK` → `unlock` (400/429 typés) ;
/// la session invitée reste une porte de sortie explicite.
class _LockedGate extends ConsumerStatefulWidget {
  const _LockedGate();

  @override
  ConsumerState<_LockedGate> createState() => _LockedGateState();
}

class _LockedGateState extends ConsumerState<_LockedGate> {
  final _password = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _password.dispose();
    super.dispose();
  }

  Future<void> _unlock() async {
    if (_password.text.isEmpty) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    final l10n = context.l10n;
    try {
      await ref
          .read(settingsRepositoryProvider)
          .identityUnlock(_password.text);
    } on ApiException catch (e) {
      if (mounted) {
        setState(
          () => _error = e.statusCode == 429
              ? l10n.gateUnlockThrottle
              : l10n.gateUnlockError,
        );
      }
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _guest() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await ref.read(settingsRepositoryProvider).identityGuest();
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Icon(
          Icons.lock_outline,
          size: 64,
          color: Theme.of(context).colorScheme.primary,
        ),
        const SizedBox(height: AppSpacing.md),
        Text(
          l10n.gateLockedTitle,
          style: Theme.of(context).textTheme.headlineSmall,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpacing.sm),
        Text(
          l10n.gateLockedBody,
          style: Theme.of(context).textTheme.bodyMedium,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: AppSpacing.lg),
        TextField(
          controller: _password,
          enabled: !_busy,
          obscureText: true,
          autofocus: true,
          decoration: InputDecoration(
            labelText: l10n.identityAtRestPasswordCurrent,
            border: const OutlineInputBorder(),
          ),
          onSubmitted: (_) => _unlock(),
        ),
        const SizedBox(height: AppSpacing.md),
        FilledButton.icon(
          onPressed: _busy ? null : _unlock,
          icon: const Icon(Icons.lock_open, size: 18),
          label: Text(l10n.gateUnlock),
        ),
        const SizedBox(height: AppSpacing.sm),
        TextButton(
          onPressed: _busy ? null : _guest,
          child: Text(l10n.gateGuestEscape),
        ),
        if (_busy) ...[
          const SizedBox(height: AppSpacing.md),
          const Center(child: CircularProgressIndicator()),
        ],
        if (_error != null) ...[
          const SizedBox(height: AppSpacing.md),
          Text(
            _error!,
            style: TextStyle(color: Theme.of(context).colorScheme.error),
            textAlign: TextAlign.center,
          ),
        ],
      ],
    );
  }
}

/// Carte de choix cliquable (gate pending).
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
