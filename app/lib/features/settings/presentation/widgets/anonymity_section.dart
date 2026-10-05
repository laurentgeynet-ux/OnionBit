// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';
import 'settings_defaults.dart';

/// Section « Tunnels anonymes » — réglages de la `TunnelCommunity`
/// (`tunnel_community/*` : activation, bornes de circuits, rôle de
/// noeud de sortie).
class AnonymitySection extends ConsumerStatefulWidget {
  const AnonymitySection({super.key});

  @override
  ConsumerState<AnonymitySection> createState() => _AnonymitySectionState();
}

class _AnonymitySectionState extends ConsumerState<AnonymitySection> {
  late final _deferred = DeferredSection(ref, 'anonymity');
  static const _t = ['tunnel_community'];

  final _minCircuits = TextEditingController();
  final _maxCircuits = TextEditingController();
  final _maxRelays = TextEditingController();
  bool _initialized = false;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _deferred.attach(save: _save, discard: _discard);
  }

  @override
  void dispose() {
    _deferred.detach();
    _minCircuits.dispose();
    _maxCircuits.dispose();
    _maxRelays.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _minCircuits.text =
        '${settingsInt(settings, [..._t, 'min_circuits'], def: 3)}';
    _maxCircuits.text =
        '${settingsInt(settings, [..._t, 'max_circuits'], def: 8)}';
    _maxRelays.text =
        '${settingsInt(settings, [..._t, 'max_joined_circuits'], def: 100)}';
    _initialized = true;
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    setState(() => _saving = true);
    final min = int.tryParse(_minCircuits.text.trim()) ?? 3;
    final max = int.tryParse(_maxCircuits.text.trim()) ?? 8;
    final relays = int.tryParse(_maxRelays.text.trim()) ?? 100;
    if (min < 0 || max < min || relays < 0) {
      setState(() => _saving = false);
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(context.l10n.anonMinMaxError)));
      return;
    }
    await applySettingsPatch(context, ref, {
      'tunnel_community': {
        'min_circuits': min,
        'max_circuits': max,
        'max_joined_circuits': relays,
      },
    }, successMessage: context.l10n.anonSaved);
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.shield_outlined,
      title: l10n.sectionAnonymity,
      sectionId: 'anonymity',
      defaults: kAnonymityDefaults,
      child: (context, settings) {
        _sync(settings);
        final enabled = settingsBool(settings, [..._t, 'enabled'], def: true);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SettingsSwitch(
              path: [..._t, 'enabled'],
              value: enabled,
              title: l10n.anonEnabled,
              subtitle: l10n.anonEnabledSub,
            ),
            SettingsSwitch(
              path: [..._t, 'exitnode_enabled'],
              value: settingsBool(settings, [..._t, 'exitnode_enabled']),
              title: l10n.anonExit,
              subtitle: l10n.anonExitSub,
            ),
            SettingsSwitch(
              path: [..._t, 'messaging_enabled'],
              value: settingsBool(settings, [
                ..._t,
                'messaging_enabled',
              ], def: true),
              title: l10n.anonMessaging,
              subtitle: l10n.anonMessagingSub,
            ),
            SettingsSwitch(
              path: [..._t, 'guards_enabled'],
              value: settingsBool(settings, [
                ..._t,
                'guards_enabled',
              ], def: true),
              title: l10n.anonGuards,
              subtitle: l10n.anonGuardsSub,
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _minCircuits,
                    onChanged: (_) => _deferred.markDirty(),
                    keyboardType: TextInputType.number,
                    decoration: InputDecoration(
                      labelText: l10n.circuitsMinLabel,
                      isDense: true,
                      suffixIcon: const KeyInfoIcon([
                        'tunnel_community',
                        'min_circuits',
                      ]),
                    ),
                  ),
                ),
                const SizedBox(width: AppSpacing.sm),
                Expanded(
                  child: TextField(
                    controller: _maxCircuits,
                    onChanged: (_) => _deferred.markDirty(),
                    keyboardType: TextInputType.number,
                    decoration: InputDecoration(
                      labelText: l10n.circuitsMaxLabel,
                      isDense: true,
                      suffixIcon: const KeyInfoIcon([
                        'tunnel_community',
                        'max_circuits',
                      ]),
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: _maxRelays,
              onChanged: (_) => _deferred.markDirty(),
              keyboardType: TextInputType.number,
              decoration: InputDecoration(
                labelText: l10n.relaysMaxLabel,
                helperText: l10n.relaysMaxNote,
                helperMaxLines: 3,
                isDense: true,
                suffixIcon: const KeyInfoIcon([
                  'tunnel_community',
                  'max_joined_circuits',
                ]),
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            // `max_relayed_rate` est auto (`-1`) : fraction de
            // l'upload mesure par l'estimateur — plus de saisie.
            Text(
              l10n.relayRateAutoNote,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: AppSpacing.xs),
            Text(
              l10n.anonCircuitsNote,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton.icon(
                onPressed: _saving ? null : _save,
                icon: const Icon(Icons.save),
                label: Text(l10n.save),
              ),
            ),
          ],
        );
      },
    );
  }
}
