// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/di/providers.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import 'settings_defaults.dart';
import 'settings_section.dart';

/// Section « Mode furtif » — transport stealth ADR-0017 : role,
/// liens d'invitation `onionbit-bridge://`, cover traffic. Tout est
/// applique au redemarrage (le transport est construit dans
/// `Ipv8Stack::start`, exclusion mutuelle avec `ipv8.enabled`).
///
/// Alerte diagnostic : des `hs1` emis sans aucune session etablie
/// trahissent souvent une derive d'horloge au-dela de la fenetre
/// d'horodatage — en zone censuree le NTP peut etre filtre ou spoofe.
class StealthSection extends ConsumerStatefulWidget {
  const StealthSection({super.key});

  @override
  ConsumerState<StealthSection> createState() => _StealthSectionState();
}

class _StealthSectionState extends ConsumerState<StealthSection> {
  late final _deferred = DeferredSection(ref, 'stealth');
  static const _s = ['stealth'];
  static const _roles = ['client', 'bridge', 'gateway'];

  final _bridges = TextEditingController();
  Timer? _poll;
  Map<String, dynamic>? _status;
  String _role = 'client';
  bool _initialized = false;

  @override
  void initState() {
    super.initState();
    _deferred.attach(save: _save, discard: _discard);
    _refreshStatus();
    _poll = Timer.periodic(const Duration(seconds: 5), (_) {
      _refreshStatus();
    });
  }

  @override
  void dispose() {
    _poll?.cancel();
    _deferred.detach();
    _bridges.dispose();
    super.dispose();
  }

  Future<void> _refreshStatus() async {
    try {
      final resp = await ref.read(apiClientProvider).get('/stealth');
      if (mounted && resp is Map<String, dynamic>) {
        setState(() => _status = resp);
      }
    } catch (_) {
      // Daemon sans stealth ou API indisponible — la section reste
      // utilisable pour la config (appliquee au redemarrage).
    }
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    final list = settingsLeaf(settings, [..._s, 'bridges']);
    if (list is List) {
      _bridges.text = list.map((e) => '$e').join('\n');
    }
    _role = '${settingsLeaf(settings, [..._s, 'role']) ?? 'client'}';
    if (!_roles.contains(_role)) _role = 'client';
    _initialized = true;
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    // Un lien `onionbit-bridge://` par ligne — la validation du
    // format est faite cote daemon au demarrage (fail-closed).
    final bridges = _bridges.text
        .split(RegExp(r'[\s,;]+'))
        .map((s) => s.trim())
        .where((s) => s.isNotEmpty)
        .toList();
    await applySettingsPatch(context, ref, {
      'stealth': {'role': _role, 'bridges': bridges},
    }, successMessage: context.l10n.stealthSaved);
    _deferred.markClean();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final status = _status;
    final metrics = status?['metrics'] as Map<String, dynamic>?;
    final sessions = (status?['sessions'] as num?)?.toInt() ?? 0;
    final hs1Sent = (metrics?['hs1_sent'] as num?)?.toInt() ?? 0;
    final hs2Ok = (metrics?['hs2_accepted'] as num?)?.toInt() ?? 0;
    // Handshakes tentes, aucune session : la cause la plus courante
    // en zone censuree est une horloge derivee (NTP filtre/spoofe)
    // qui sort l'horodatage hs1 de la fenetre acceptee.
    final clockSuspect =
        status != null &&
        status['role'] == 'client' &&
        sessions == 0 &&
        hs1Sent > 3 &&
        hs2Ok == 0;
    return SettingsSection(
      icon: Icons.visibility_off_outlined,
      title: l10n.sectionStealth,
      sectionId: 'stealth',
      defaults: kStealthDefaults,
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SettingsSwitch(
              path: [..._s, 'enabled'],
              value: settingsBool(settings, [..._s, 'enabled']),
              title: l10n.stealthEnabled,
              subtitle: l10n.stealthEnabledSub,
            ),
            const SizedBox(height: AppSpace.sm),
            DropdownButtonFormField<String>(
              initialValue: _role,
              decoration: InputDecoration(
                labelText: l10n.stealthRoleLabel,
                suffixIcon: const KeyInfoIcon(['stealth', 'role']),
              ),
              items: [
                for (final r in _roles)
                  DropdownMenuItem(
                    value: r,
                    child: Text(switch (r) {
                      'client' => l10n.stealthRoleClient,
                      'bridge' => l10n.stealthRoleBridge,
                      _ => l10n.stealthRoleGateway,
                    }),
                  ),
              ],
              onChanged: (v) => setState(() {
                _role = v ?? 'client';
                _deferred.markDirty();
              }),
            ),
            const SizedBox(height: AppSpace.sm),
            TextField(
              controller: _bridges,
              maxLines: 4,
              minLines: 2,
              decoration: InputDecoration(
                labelText: l10n.stealthBridgesLabel,
                helperText: l10n.stealthBridgesNote,
                alignLabelWithHint: true,
                border: const OutlineInputBorder(),
              ),
              style: theme.textTheme.bodySmall?.copyWith(
                fontFamily: 'monospace',
              ),
              onChanged: (_) => _deferred.markDirty(),
            ),
            const SizedBox(height: AppSpace.sm),
            SettingsSwitch(
              path: [..._s, 'cover_traffic'],
              value: settingsBool(settings, [..._s, 'cover_traffic']),
              title: l10n.stealthCover,
              subtitle: l10n.stealthCoverSub,
            ),
            if (status != null) ...[
              const Divider(height: AppSpace.lg),
              Text(
                l10n.stealthStatus(
                  '${status['role']}',
                  '$sessions',
                  '${(status['bridges_configured'] as num?)?.toInt() ?? 0}',
                ),
                style: theme.textTheme.bodySmall,
              ),
              if (clockSuspect) ...[
                const SizedBox(height: AppSpace.sm),
                Card(
                  color: theme.colorScheme.errorContainer,
                  child: Padding(
                    padding: const EdgeInsets.all(AppSpace.sm),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          Icons.schedule_outlined,
                          size: 20,
                          color: theme.colorScheme.onErrorContainer,
                        ),
                        const SizedBox(width: AppSpace.sm),
                        Expanded(
                          child: Text(
                            l10n.stealthClockWarning,
                            style: theme.textTheme.bodySmall?.copyWith(
                              color: theme.colorScheme.onErrorContainer,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ],
          ],
        );
      },
    );
  }
}
