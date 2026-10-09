// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import '../../../../l10n/app_localizations.dart' show AppLocalizations;
import 'settings_section.dart';
import 'settings_defaults.dart';

/// Section « Seed & anonymat par défaut » — politique de seed des
/// nouveaux téléchargements (`libtorrent/download_defaults/*` :
/// `seeding_mode`, `seeding_ratio`, `seeding_time`,
/// `safeseeding_enabled`, `anonymity_enabled`, `number_hops`).
class SeedingSection extends ConsumerStatefulWidget {
  const SeedingSection({super.key});

  @override
  ConsumerState<SeedingSection> createState() => _SeedingSectionState();
}

class _SeedingSectionState extends ConsumerState<SeedingSection> {
  late final _deferred = DeferredSection(ref, 'seeding');
  static const _dd = ['libtorrent', 'download_defaults'];

  /// Modes de seed du backend (`forever`/`never`/`ratio`/`time`) —
  /// les libellés affichés sont localisés via [_modeLabel].
  static const _modeKeys = ['forever', 'never', 'ratio', 'time'];

  /// Libellé localisé d'un mode de seed backend.
  static String _modeLabel(AppLocalizations l10n, String mode) =>
      switch (mode) {
        'forever' => l10n.seedModeForever,
        'never' => l10n.seedModeNever,
        'ratio' => l10n.seedModeRatio,
        'time' => l10n.seedModeTime,
        _ => mode,
      };

  String _mode = 'forever';
  int _hops = 0;
  bool _anonymity = false;
  bool _safeSeeding = false;
  final _ratio = TextEditingController();
  final _time = TextEditingController();
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
    _ratio.dispose();
    _time.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _mode = settingsString(settings, [..._dd, 'seeding_mode'], def: 'forever');
    if (!_modeKeys.contains(_mode)) _mode = 'forever';
    _anonymity = settingsBool(settings, [..._dd, 'anonymity_enabled']);
    _hops = settingsInt(settings, [..._dd, 'number_hops']).clamp(0, 3);
    _safeSeeding = settingsBool(settings, [..._dd, 'safeseeding_enabled']);
    _ratio.text = '${settingsDouble(settings, [..._dd, 'seeding_ratio'])}';
    _time.text = '${settingsInt(settings, [..._dd, 'seeding_time'])}';
    _initialized = true;
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    setState(() => _saving = true);
    await applySettingsPatch(context, ref, {
      'libtorrent': {
        'download_defaults': {
          'seeding_mode': _mode,
          'seeding_ratio': double.tryParse(_ratio.text.trim()) ?? 0,
          'seeding_time': double.tryParse(_time.text.trim()) ?? 0,
          'safeseeding_enabled': _safeSeeding,
          'anonymity_enabled': _anonymity,
          // Règle backend : `number_hops > 0` impose le safe seeding.
          'number_hops': _hops,
        },
      },
    }, successMessage: context.l10n.seedSaved);
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.upload,
      title: l10n.sectionSeeding,
      sectionId: 'seeding',
      defaults: kSeedingDefaults,
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(l10n.seedMode, style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpace.xs),
            SegmentedButton<String>(
              segments: [
                for (final m in _modeKeys)
                  ButtonSegment(value: m, label: Text(_modeLabel(l10n, m))),
              ],
              selected: {_mode},
              onSelectionChanged: (s) => setState(() {
                _mode = s.first;
                _deferred.markDirty();
              }),
            ),
            const SizedBox(height: AppSpace.xs),
            Text(
              l10n.seedModeDefault,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: AppSpace.sm),
            if (_mode == 'ratio')
              TextField(
                controller: _ratio,
                onChanged: (_) => _deferred.markDirty(),
                keyboardType: const TextInputType.numberWithOptions(
                  decimal: true,
                ),
                decoration: InputDecoration(
                  labelText: l10n.seedRatioLabel,
                  hintText: l10n.defaultHint('2.0'),
                  suffixIcon: const KeyInfoIcon([
                    'libtorrent',
                    'download_defaults',
                    'seeding_ratio',
                  ]),
                ),
              ),
            if (_mode == 'time')
              TextField(
                controller: _time,
                onChanged: (_) => _deferred.markDirty(),
                keyboardType: TextInputType.number,
                decoration: InputDecoration(
                  labelText: l10n.seedTimeLabel,
                  hintText: l10n.defaultHint('60'),
                  suffixIcon: const KeyInfoIcon([
                    'libtorrent',
                    'download_defaults',
                    'seeding_time',
                  ]),
                ),
              ),
            const Divider(height: AppSpace.lg),
            Text(l10n.seedAnonTitle, style: theme.textTheme.labelMedium),
            SettingsSwitch(
              path: [..._dd, 'anonymity_enabled'],
              value: _anonymity,
              title: l10n.seedAnonSwitch,
              subtitle: l10n.seedAnonSub,
              onChangedOverride: (v) => setState(() {
                _deferred.markDirty();
                _anonymity = v;
                // Anonyme implique ≥1 saut — cohérence du couple
                // `anonymity_enabled`/`number_hops`.
                if (v && _hops == 0) _hops = 1;
              }),
            ),
            if (_anonymity) ...[
              const SizedBox(height: AppSpace.xs),
              SegmentedButton<int>(
                segments: [
                  for (final n in const [1, 2, 3])
                    ButtonSegment(value: n, label: Text(l10n.ctxHops(n))),
                ],
                selected: {_hops == 0 ? 1 : _hops},
                onSelectionChanged: (s) => setState(() {
                  _hops = s.first;
                  _deferred.markDirty();
                }),
              ),
              SwitchListTile(
                value: _safeSeeding || _hops > 0,
                // `number_hops > 0` impose `safeseeding` — invariant
                // backend, le commutateur est donc forcé à true.
                onChanged: _hops > 0
                    ? null
                    : (v) => setState(() {
                        _safeSeeding = v;
                        _deferred.markDirty();
                      }),
                title: const Text('Safe seeding'),
                subtitle: Text(l10n.safeSeedingSub),
                contentPadding: EdgeInsets.zero,
                dense: true,
              ),
            ],
            const SizedBox(height: AppSpace.sm),
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
