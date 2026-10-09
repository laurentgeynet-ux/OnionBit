// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import 'settings_defaults.dart';
import 'settings_section.dart';

/// Section « OnionBit » — mécanismes spécifiques au protocole
/// d'extension (ADR-0015) : communauté ext (hello signé, capacités,
/// registre bilatéral, curation, obfuscation) et comptabilité tunnel.
/// Les commutateurs `ext/*` construisent la communauté au démarrage
/// → appliqués au redémarrage ; les commutateurs `tunnel_community/
/// ledger_*` sont rechargés à chaud par `apply_service_settings`.
class OnionBitSection extends ConsumerStatefulWidget {
  const OnionBitSection({super.key});

  @override
  ConsumerState<OnionBitSection> createState() => _OnionBitSectionState();
}

class _OnionBitSectionState extends ConsumerState<OnionBitSection> {
  late final _deferred = DeferredSection(ref, 'onionbit');
  static const _e = ['ext'];
  static const _t = ['tunnel_community'];

  final _curators = TextEditingController();
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
    _curators.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    final list = settingsLeaf(settings, [..._e, 'curators']);
    if (list is List) {
      _curators.text = list.map((e) => '$e').join(', ');
    }
    _initialized = true;
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    setState(() => _saving = true);
    // Clés publiques hex séparées par virgule/espace/retour — les
    // entrées invalides sont ignorées côté daemon (logguées).
    final curators = _curators.text
        .split(RegExp(r'[\s,;]+'))
        .map((s) => s.trim())
        .where((s) => s.isNotEmpty)
        .toList();
    await applySettingsPatch(context, ref, {
      'ext': {'curators': curators},
    }, successMessage: context.l10n.onionbitSaved);
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.hub_outlined,
      title: l10n.sectionOnionBit,
      sectionId: 'onionbit',
      defaults: kOnionBitDefaults,
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SettingsSwitch(
              path: [..._e, 'enabled'],
              value: settingsBool(settings, [..._e, 'enabled'], def: true),
              title: l10n.onionbitExtEnabled,
              subtitle: l10n.onionbitExtEnabledSub,
            ),
            SettingsSwitch(
              path: [..._e, 'ledger_enabled'],
              value: settingsBool(
                settings,
                [..._e, 'ledger_enabled'],
                def: true,
              ),
              title: l10n.onionbitExtLedger,
              subtitle: l10n.onionbitExtLedgerSub,
            ),
            SettingsSwitch(
              path: [..._e, 'obf_enabled'],
              value: settingsBool(settings, [..._e, 'obf_enabled']),
              title: l10n.onionbitObf,
              subtitle: l10n.onionbitObfSub,
            ),
            const Divider(height: AppSpace.lg),
            // Comptabilité tunnel (ADR-0015 §3) — rechargée à chaud.
            SettingsSwitch(
              path: [..._t, 'ledger_enabled'],
              value: settingsBool(
                settings,
                [..._t, 'ledger_enabled'],
                def: true,
              ),
              title: l10n.onionbitLedgerCollect,
              subtitle: l10n.onionbitLedgerCollectSub,
            ),
            SettingsSwitch(
              path: [..._t, 'ledger_enforce'],
              value: settingsBool(settings, [..._t, 'ledger_enforce']),
              title: l10n.onionbitLedgerEnforce,
              subtitle: l10n.onionbitLedgerEnforceSub,
            ),
            const Divider(height: AppSpace.lg),
            // Consentement messagerie assisté par la confiance
            // ADR-0015 (attestations `identity` + solde du registre
            // tunnel) — politique locale, appliquée au redémarrage.
            SettingsSwitch(
              path: [..._t, 'messaging_consent_endorsed'],
              value: settingsBool(
                settings,
                [..._t, 'messaging_consent_endorsed'],
              ),
              title: l10n.onionbitConsentEndorsed,
              subtitle: l10n.onionbitConsentEndorsedSub,
            ),
            SettingsSwitch(
              path: [..._t, 'messaging_consent_flagged'],
              value: settingsBool(
                settings,
                [..._t, 'messaging_consent_flagged'],
              ),
              title: l10n.onionbitConsentFlagged,
              subtitle: l10n.onionbitConsentFlaggedSub,
            ),
            SettingsSwitch(
              path: [..._t, 'messaging_consent_ledger'],
              value: settingsBool(
                settings,
                [..._t, 'messaging_consent_ledger'],
              ),
              title: l10n.onionbitConsentLedger,
              subtitle: l10n.onionbitConsentLedgerSub,
            ),
            const Divider(height: AppSpace.lg),
            TextField(
              controller: _curators,
              onChanged: (_) => _deferred.markDirty(),
              minLines: 1,
              maxLines: 3,
              decoration: InputDecoration(
                labelText: l10n.onionbitCurators,
                helperText: l10n.onionbitCuratorsNote,
                helperMaxLines: 3,
                isDense: true,
                suffixIcon: const KeyInfoIcon(['ext', 'curators']),
              ),
            ),
            const SizedBox(height: AppSpace.sm),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton.icon(
                onPressed: _saving ? null : _save,
                icon: const Icon(Icons.save),
                label: Text(l10n.save),
              ),
            ),
            const SizedBox(height: AppSpace.xs),
            Text(
              l10n.onionbitNote,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        );
      },
    );
  }
}
