// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/api_client.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../core/di/providers.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../l10n/app_localizations.dart';
import '../../domain/privacy_profile.dart';
import '../../domain/privacy_repository.dart';
import '../providers/privacy_providers.dart';

/// Sélecteur de profil d'anonymat (ADR-0022 §5) — trois positions
/// matérialisant les presets `legacy` / `full` / `custom` dans la
/// sidebar (dépliée : lignes pilule ; rail : menu contextuel sur
/// icône). Le segment sélectionné reflète le profil **effectif**
/// dérivé par le daemon — modifier une clé couverte à la main bascule
/// automatiquement l'affichage sur « Personnalisé ».
///
/// Bascules : `full` ouvre le dialogue de conséquences (interop
/// Tribler sacrifiée, redémarrage) avec saisie de pont inline quand
/// aucun n'est configuré ; retour `legacy` depuis `full` → dialogue
/// allégé ; `custom` → bascule directe (aucune clé réécrite).
/// `restart_pending` affiche une puce « redémarrage en attente » qui
/// rouvre le dialogue de redémarrage.
class PrivacyProfileSwitch extends ConsumerWidget {
  const PrivacyProfileSwitch({super.key, this.collapsed = false});

  /// Mode rail (sidebar rétractée) : icône seule + menu contextuel.
  final bool collapsed;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final async = ref.watch(privacyProfileProvider);
    return switch (async) {
      AsyncData(:final value) => collapsed
          ? _ProfileRailMenu(state: value)
          : _ProfileOptions(state: value),
      AsyncError() => collapsed
          ? const _ProfileRailMenu(state: null)
          : _ProfileErrorTile(
              onRetry: () => ref.invalidate(privacyProfileProvider),
            ),
      _ => collapsed
          ? const _ProfileRailMenu(state: null)
          : const _ProfileOptions(state: null),
    };
  }
}

/// Icône / label / description par profil — centralisé pour garder
/// menu rail, lignes et dialogues sur le même copy deck (ADR §5).
extension on PrivacyProfileKind {
  IconData get icon => switch (this) {
    PrivacyProfileKind.legacy => Icons.lock_outline,
    PrivacyProfileKind.full => Icons.enhanced_encryption_outlined,
    PrivacyProfileKind.custom => Icons.tune,
  };

  String label(AppLocalizations l10n) => switch (this) {
    PrivacyProfileKind.legacy => l10n.privacyProfileLegacy,
    PrivacyProfileKind.full => l10n.privacyProfileFull,
    PrivacyProfileKind.custom => l10n.privacyProfileCustom,
  };

  String description(AppLocalizations l10n) => switch (this) {
    PrivacyProfileKind.legacy => l10n.privacyProfileLegacyDesc,
    PrivacyProfileKind.full => l10n.privacyProfileFullDesc,
    PrivacyProfileKind.custom => l10n.privacyProfileCustomDesc,
  };
}

// ------------------------------------------------------------------
// Vue dépliée : trois lignes pilule (style `_SidebarItem`).
// ------------------------------------------------------------------

class _ProfileOptions extends ConsumerWidget {
  const _ProfileOptions({required this.state});

  /// `null` pendant le chargement : lignes grisées sans sélection.
  final PrivacyProfileState? state;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final guest = state?.guest ?? false;
    final enabled = state != null && !guest;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final kind in PrivacyProfileKind.values)
          _ProfileOptionRow(
            kind: kind,
            selected: state?.effective == kind,
            enabled: enabled,
            tooltip: guest
                ? '${kind.label(l10n)} — ${l10n.privacyProfileGuestLocked}'
                : '${kind.label(l10n)} — ${kind.description(l10n)}',
            onTap: enabled
                ? () => requestProfileSwitch(context, ref, state, kind)
                : null,
          ),
        if (state?.restartPending ?? false) const _RestartPendingChip(),
      ],
    );
  }
}

class _ProfileOptionRow extends StatelessWidget {
  const _ProfileOptionRow({
    required this.kind,
    required this.selected,
    required this.enabled,
    required this.tooltip,
    this.onTap,
  });

  final PrivacyProfileKind kind;
  final bool selected;
  final bool enabled;
  final String tooltip;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final color = selected
        ? scheme.onPrimaryContainer
        : enabled
        ? scheme.onSurface
        : scheme.outline;
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: Tooltip(
        message: tooltip,
        child: InkWell(
          borderRadius: BorderRadius.circular(24),
          onTap: onTap,
          child: Container(
            decoration: BoxDecoration(
              color: selected ? scheme.primaryContainer : Colors.transparent,
              borderRadius: BorderRadius.circular(24),
            ),
            padding: const EdgeInsets.symmetric(
              horizontal: AppSpace.sm,
              vertical: AppSpace.sm - 2,
            ),
            child: Row(
              children: [
                Icon(kind.icon, size: 18, color: color),
                const SizedBox(width: AppSpace.sm),
                Expanded(
                  child: Text(
                    kind.label(context.l10n),
                    style: theme.textTheme.bodyMedium?.copyWith(
                      color: color,
                      fontWeight: selected
                          ? FontWeight.w600
                          : FontWeight.normal,
                    ),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (selected && kind == PrivacyProfileKind.custom)
                  Icon(
                    Icons.info_outline,
                    size: 14,
                    color: scheme.onPrimaryContainer,
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Erreur de chargement du profil — tuile discrète avec relance.
class _ProfileErrorTile extends StatelessWidget {
  const _ProfileErrorTile({required this.onRetry});

  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Tooltip(
      message: context.l10n.privacyProfileLoadError,
      child: InkWell(
        borderRadius: BorderRadius.circular(24),
        onTap: onRetry,
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpace.sm,
            vertical: AppSpace.sm - 2,
          ),
          child: Row(
            children: [
              Icon(Icons.error_outline, size: 18, color: scheme.error),
              const SizedBox(width: AppSpace.sm),
              Expanded(
                child: Text(
                  context.l10n.privacyProfileTitle,
                  style: Theme.of(
                    context,
                  ).textTheme.bodyMedium?.copyWith(color: scheme.outline),
                ),
              ),
              Icon(Icons.refresh, size: 16, color: scheme.outline),
            ],
          ),
        ),
      ),
    );
  }
}

/// Puce « redémarrage en attente » — le preset persisté n'est pas
/// encore le régime actif ; rouvre le dialogue de redémarrage.
class _RestartPendingChip extends ConsumerWidget {
  const _RestartPendingChip();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final scheme = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.only(top: AppSpace.xs),
      child: Material(
        color: scheme.tertiaryContainer,
        borderRadius: BorderRadius.circular(AppRadius.small),
        child: InkWell(
          borderRadius: BorderRadius.circular(AppRadius.small),
          onTap: () => _RestartDialog.show(context, ref),
          child: Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: AppSpace.sm,
              vertical: AppSpace.xs,
            ),
            child: Row(
              children: [
                Icon(
                  Icons.restart_alt,
                  size: 16,
                  color: scheme.onTertiaryContainer,
                ),
                const SizedBox(width: AppSpace.xs),
                Expanded(
                  child: Text(
                    context.l10n.privacyProfileRestartPending,
                    style: Theme.of(context).textTheme.labelSmall?.copyWith(
                      color: scheme.onTertiaryContainer,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------
// Vue rail : icône seule + menu contextuel des trois positions.
// ------------------------------------------------------------------

class _ProfileRailMenu extends ConsumerWidget {
  const _ProfileRailMenu({required this.state});

  /// `null` : chargement ou erreur — icône neutre désactivée.
  final PrivacyProfileState? state;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final effective = state?.effective;
    final guest = state?.guest ?? false;
    final enabled = state != null && !guest;
    final icon = effective?.icon ?? Icons.privacy_tip_outlined;
    final tooltip = state == null
        ? l10n.privacyProfileTitle
        : guest
        ? l10n.privacyProfileGuestLocked
        : '${l10n.privacyProfileTitle} : ${effective!.label(l10n)} — '
              '${effective.description(l10n)}';
    return Tooltip(
      message: tooltip,
      child: Center(
        child: PopupMenuButton<PrivacyProfileKind>(
          enabled: enabled,
          tooltip: tooltip,
          position: PopupMenuPosition.over,
          onSelected: (kind) =>
              requestProfileSwitch(context, ref, state, kind),
          itemBuilder: (ctx) => [
            for (final kind in PrivacyProfileKind.values)
              PopupMenuItem<PrivacyProfileKind>(
                value: kind,
                child: ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  leading: Icon(kind.icon, size: 20),
                  title: Text(kind.label(ctx.l10n)),
                  subtitle: Text(
                    kind.description(ctx.l10n),
                    style: Theme.of(ctx).textTheme.bodySmall,
                  ),
                  trailing: state?.effective == kind
                      ? const Icon(Icons.check, size: 18)
                      : null,
                ),
              ),
          ],
          child: Padding(
            padding: const EdgeInsets.all(AppSpace.sm - 2),
            child: Badge(
              isLabelVisible: state?.restartPending ?? false,
              smallSize: 8,
              child: Icon(
                icon,
                size: 20,
                color: enabled
                    ? Theme.of(context).colorScheme.onSurface
                    : Theme.of(context).colorScheme.outline,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------
// Flux de bascule : dialogues, prérequis pont, redémarrage.
// ------------------------------------------------------------------

/// Point d'entrée commun (lignes dépliées, menu rail, feuille HUD) :
/// sélection d'une position → dialogue adapté → `PUT /privacy/profile`
/// → invalidation du provider → dialogue de redémarrage si requis.
Future<void> requestProfileSwitch(
  BuildContext context,
  WidgetRef ref,
  PrivacyProfileState? state,
  PrivacyProfileKind target,
) async {
  if (state == null || target == state.effective) return;
  final repo = ref.read(privacyRepositoryProvider);
  final l10n = context.l10n;
  try {
    // La future est construite synchrone (le `context` n'est pas
    // utilisé après un `await`) puis awaitée une seule fois : retour
    // `legacy` depuis `full` → dialogue allégé ; `full` → dialogue de
    // conséquences ; `custom` → bascule directe.
    final action = switch (target) {
      PrivacyProfileKind.custom => repo.switchProfile(target),
      PrivacyProfileKind.legacy =>
        state.effective == PrivacyProfileKind.full
            ? _ReturnLegacyDialog.show(context, repo)
            : repo.switchProfile(target),
      PrivacyProfileKind.full => _FullConsequencesDialog.show(
        context,
        repo,
        state,
      ),
    };
    final result = await action;
    if (result == null || !context.mounted) return;
    ref.invalidate(privacyProfileProvider);
    if (result.restartRequired) await _RestartDialog.show(context, ref);
  } on ApiException catch (e) {
    if (context.mounted) {
      final message = e.statusCode == 409 && e.message == 'guest_session'
          ? l10n.privacyProfileGuestLocked
          : l10n.privacyProfileSwitchError(e.message);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(message)));
    }
  } catch (e) {
    if (context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(l10n.privacyProfileSwitchError('$e'))),
      );
    }
  }
}

/// Dialogue « Activer la Protection max ? » — conséquences
/// (interop Tribler sacrifiée, redémarrage) + saisie inline du lien
/// `onionbit-bridge://` quand aucun pont n'est configuré : le
/// prérequis serveur `stealth.bridges` est satisfait juste avant la
/// bascule (`POST /stealth/bridges` → `PUT /privacy/profile`).
class _FullConsequencesDialog extends StatefulWidget {
  const _FullConsequencesDialog({required this.repo, required this.state});

  /// Le dialogue vit dans une nouvelle route — le dépôt est capturé
  /// explicitement plutôt que relu via `ref`.
  final PrivacyRepository repo;
  final PrivacyProfileState state;

  /// Renvoie le résultat de la bascule, `null` si annulé.
  static Future<PrivacySwitchResult?> show(
    BuildContext context,
    PrivacyRepository repo,
    PrivacyProfileState state,
  ) => showDialog<PrivacySwitchResult>(
    context: context,
    builder: (_) => _FullConsequencesDialog(repo: repo, state: state),
  );

  @override
  State<_FullConsequencesDialog> createState() =>
      _FullConsequencesDialogState();
}

class _FullConsequencesDialogState extends State<_FullConsequencesDialog> {
  final _bridge = TextEditingController();
  bool _busy = false;
  String? _error;

  bool get _needsBridge => widget.state.bridgesConfigured == 0;

  @override
  void dispose() {
    _bridge.dispose();
    super.dispose();
  }

  Future<void> _confirm() async {
    final l10n = context.l10n;
    final repo = widget.repo;
    final link = _bridge.text.trim();
    if (_needsBridge) {
      if (!link.startsWith('onionbit-bridge://')) {
        setState(() => _error = l10n.privacyProfileBridgeInvalid);
        return;
      }
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      if (link.isNotEmpty) await repo.addBridge(link);
      final result = await repo.switchProfile(PrivacyProfileKind.full);
      if (mounted) Navigator.of(context).pop(result);
    } on ApiException catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = e.statusCode == 409 &&
                  e.message == 'missing_prerequisites'
              ? l10n.privacyProfileBridgeInvalid
              : e.message;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _busy = false);
      rethrow;
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return AlertDialog(
      title: Text(l10n.privacyProfileFullTitle),
      scrollable: true,
      content: SizedBox(
        width: 440,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(l10n.privacyProfileFullBody),
            if (_needsBridge) ...[
              const SizedBox(height: AppSpace.md),
              Text(l10n.privacyProfileBridgeHint),
              const SizedBox(height: AppSpace.xs),
              TextField(
                controller: _bridge,
                autofocus: true,
                decoration: InputDecoration(
                  labelText: l10n.privacyProfileBridgeFieldLabel,
                  hintText: 'onionbit-bridge://…',
                  errorText: _error,
                  isDense: true,
                ),
              ),
            ] else if (_error != null) ...[
              const SizedBox(height: AppSpace.sm),
              Text(
                _error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: _busy ? null : _confirm,
          child: Text(
            _needsBridge
                ? l10n.privacyProfileAddBridgeAndActivate
                : l10n.privacyProfileActivate,
          ),
        ),
      ],
    );
  }
}

/// Dialogue « Revenir au mode Compatible ? » — reconnexion au mesh
/// Tribler (le transport furtif est désactivé, les ponts conservés).
class _ReturnLegacyDialog extends StatelessWidget {
  const _ReturnLegacyDialog({required this.repo});

  final PrivacyRepository repo;

  static Future<PrivacySwitchResult?> show(
    BuildContext context,
    PrivacyRepository repo,
  ) => showDialog<PrivacySwitchResult>(
    context: context,
    builder: (_) => _ReturnLegacyDialog(repo: repo),
  );

  Future<void> _confirm(BuildContext context) async {
    final result = await repo.switchProfile(PrivacyProfileKind.legacy);
    if (context.mounted) Navigator.of(context).pop(result);
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return AlertDialog(
      title: Text(l10n.privacyProfileLegacyTitle),
      content: SizedBox(
        width: 440,
        child: Text(l10n.privacyProfileLegacyBody),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: () => _confirm(context),
          child: Text(l10n.privacyProfileActivate),
        ),
      ],
    );
  }
}

/// Dialogue de redémarrage — la bascule est persistée mais les clés
/// froides attendent le prochain démarrage. Daemon local (loopback,
/// non-web) : « Redémarrer maintenant » envoie `PUT /api/shutdown`,
/// la reconnexion respawn via `ensureDaemonRunning` — cycle existant
/// de la bannière « daemon injoignable ». Daemon distant / web :
/// redémarrage manuel sur sa machine.
class _RestartDialog extends StatelessWidget {
  const _RestartDialog({required this.localDaemon});

  /// Le daemon piloté est local : l'app peut proposer le redémarrage
  /// automatique (shutdown → respawn à la reconnexion).
  final bool localDaemon;

  static Future<void> show(BuildContext context, WidgetRef ref) {
    final url = ref.read(appConfigProvider).baseUrl;
    final host = Uri.tryParse(url)?.host ?? '';
    final loopback =
        host == '127.0.0.1' ||
        host == 'localhost' ||
        host == '::1' ||
        host == '[::1]';
    return showDialog(
      context: context,
      builder: (_) =>
          _RestartDialog(localDaemon: !kIsWeb && loopback),
    );
  }

  Future<void> _restartNow(BuildContext context, WidgetRef ref) async {
    final repo = ref.read(privacyRepositoryProvider);
    try {
      await repo.shutdown();
    } catch (_) {
      // Le daemon coupe la connexion avant/après la réponse — un
      // échec de requête ne signifie pas que l'arrêt a échoué.
    }
    if (context.mounted) {
      Navigator.of(context).pop();
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(context.l10n.privacyProfileRestarting)),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return Consumer(
      builder: (context, ref, _) => AlertDialog(
        title: Text(l10n.privacyProfileRestartTitle),
        content: SizedBox(
          width: 400,
          child: Text(
            localDaemon
                ? l10n.privacyProfileRestartBody
                : l10n.privacyProfileRestartBodyRemote,
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: Text(
              localDaemon ? l10n.privacyProfileRestartLater : l10n.close,
            ),
          ),
          if (localDaemon)
            FilledButton.icon(
              onPressed: () => _restartNow(context, ref),
              icon: const Icon(Icons.restart_alt, size: 18),
              label: Text(l10n.privacyProfileRestartNow),
            ),
        ],
      ),
    );
  }
}
