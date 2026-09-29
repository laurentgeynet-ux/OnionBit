import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';

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
  static const _dd = ['libtorrent', 'download_defaults'];

  /// Modes de seed du backend (`forever`/`never`/`ratio`/`time`).
  static const _modes = {
    'forever': 'Pour toujours',
    'never': 'Jamais',
    'ratio': 'Jusqu\'au ratio',
    'time': 'Durée limitée',
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
  void dispose() {
    _ratio.dispose();
    _time.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _mode = settingsString(
      settings,
      [..._dd, 'seeding_mode'],
      def: 'forever',
    );
    if (!_modes.containsKey(_mode)) _mode = 'forever';
    _anonymity = settingsBool(settings, [..._dd, 'anonymity_enabled']);
    _hops = settingsInt(settings, [..._dd, 'number_hops']).clamp(0, 3);
    _safeSeeding = settingsBool(settings, [..._dd, 'safeseeding_enabled']);
    _ratio.text = '${settingsDouble(settings, [..._dd, 'seeding_ratio'])}';
    _time.text = '${settingsInt(settings, [..._dd, 'seeding_time'])}';
    _initialized = true;
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    await applySettingsPatch(
      context,
      ref,
      {
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
      },
      successMessage: 'Politique de seed enregistrée',
    );
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SettingsSection(
      icon: Icons.upload,
      title: 'Seed & anonymat par défaut',
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Mode de seed', style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpacing.xs),
            SegmentedButton<String>(
              segments: [
                for (final e in _modes.entries)
                  ButtonSegment(value: e.key, label: Text(e.value)),
              ],
              selected: {_mode},
              onSelectionChanged: (s) => setState(() => _mode = s.first),
            ),
            const SizedBox(height: AppSpacing.xs),
            Text(
              'Défaut : « Pour toujours ».',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            if (_mode == 'ratio')
              TextField(
                controller: _ratio,
                keyboardType: const TextInputType.numberWithOptions(
                  decimal: true,
                ),
                decoration: const InputDecoration(
                  labelText: 'Ratio de seed cible',
                  hintText: 'défaut : 2.0',
                ),
              ),
            if (_mode == 'time')
              TextField(
                controller: _time,
                keyboardType: TextInputType.number,
                decoration: const InputDecoration(
                  labelText: 'Durée de seed (secondes)',
                  hintText: 'défaut : 60',
                ),
              ),
            const Divider(height: AppSpacing.lg),
            Text(
              'Anonymat par défaut des nouveaux téléchargements',
              style: theme.textTheme.labelMedium,
            ),
            SettingsSwitch(
              path: [..._dd, 'anonymity_enabled'],
              value: _anonymity,
              title: 'Téléchargements anonymes par défaut',
              subtitle: 'Défaut : activé, 1 saut.',
              onChangedOverride: (v) => setState(() {
                _anonymity = v;
                // Anonyme implique ≥1 saut — cohérence du couple
                // `anonymity_enabled`/`number_hops`.
                if (v && _hops == 0) _hops = 1;
              }),
            ),
            if (_anonymity) ...[
              const SizedBox(height: AppSpacing.xs),
              SegmentedButton<int>(
                segments: const [
                  ButtonSegment(value: 1, label: Text('1 saut')),
                  ButtonSegment(value: 2, label: Text('2 sauts')),
                  ButtonSegment(value: 3, label: Text('3 sauts')),
                ],
                selected: {_hops == 0 ? 1 : _hops},
                onSelectionChanged: (s) => setState(() => _hops = s.first),
              ),
              SwitchListTile(
                value: _safeSeeding || _hops > 0,
                // `number_hops > 0` impose `safeseeding` — invariant
                // backend, le commutateur est donc forcé à true.
                onChanged: _hops > 0
                    ? null
                    : (v) => setState(() => _safeSeeding = v),
                title: const Text('Safe seeding'),
                subtitle: const Text(
                  'Obligatoire quand des sauts anonymes sont demandés. '
                  'Défaut : activé.',
                ),
                contentPadding: EdgeInsets.zero,
                dense: true,
              ),
            ],
            const SizedBox(height: AppSpacing.sm),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton.icon(
                onPressed: _saving ? null : _save,
                icon: const Icon(Icons.save),
                label: const Text('Enregistrer'),
              ),
            ),
          ],
        );
      },
    );
  }
}
