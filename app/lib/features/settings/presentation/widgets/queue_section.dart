import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';

/// Section « File d'attente » — bornes du gestionnaire de file
/// (`libtorrent/active_*` : `-1` = illimité) et comportement
/// `auto_managed` par défaut des nouveaux téléchargements.
class QueueSection extends ConsumerStatefulWidget {
  const QueueSection({super.key});

  @override
  ConsumerState<QueueSection> createState() => _QueueSectionState();
}

class _QueueSectionState extends ConsumerState<QueueSection> {
  final _downloads = TextEditingController();
  final _seeds = TextEditingController();
  final _checking = TextEditingController();
  final _limit = TextEditingController();
  bool _initialized = false;
  bool _saving = false;

  @override
  void dispose() {
    _downloads.dispose();
    _seeds.dispose();
    _checking.dispose();
    _limit.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    int at(String k) => settingsInt(settings, ['libtorrent', k], def: -1);
    _downloads.text = '${at('active_downloads')}';
    _seeds.text = '${at('active_seeds')}';
    _checking.text = '${at('active_checking')}';
    _limit.text = '${at('active_limit')}';
    _initialized = true;
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    int parse(String s) => int.tryParse(s.trim()) ?? -1;
    await applySettingsPatch(
      context,
      ref,
      {
        'libtorrent': {
          'active_downloads': parse(_downloads.text),
          'active_seeds': parse(_seeds.text),
          'active_checking': parse(_checking.text),
          'active_limit': parse(_limit.text),
        },
      },
      successMessage: 'File d\'attente enregistrée',
    );
    if (mounted) setState(() => _saving = false);
  }

  Widget _field(TextEditingController c, String label) => Expanded(
    child: TextField(
      controller: c,
      keyboardType: TextInputType.number,
      decoration: InputDecoration(
        labelText: label,
        hintText: '-1 = illimité',
        isDense: true,
      ),
    ),
  );

  @override
  Widget build(BuildContext context) {
    return SettingsSection(
      icon: Icons.queue,
      title: 'File d\'attente',
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Les téléchargements « gérés automatiquement » au-delà de '
              'ces bornes sont mis en pause puis repris quand un slot '
              'se libère.',
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                color: Theme.of(context).colorScheme.outline,
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                _field(_downloads, 'Téléchargements actifs'),
                const SizedBox(width: AppSpacing.sm),
                _field(_seeds, 'Seeds actifs'),
              ],
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                _field(_checking, 'Vérifications actives'),
                const SizedBox(width: AppSpacing.sm),
                _field(_limit, 'Limite globale'),
              ],
            ),
            SettingsSwitch(
              path: const [
                'libtorrent',
                'download_defaults',
                'auto_managed',
              ],
              value: settingsBool(
                settings,
                const ['libtorrent', 'download_defaults', 'auto_managed'],
              ),
              title: 'Gestion automatique par défaut',
              subtitle:
                  'Les nouveaux téléchargements sont placés sous la '
                  'file d\'attente.',
            ),
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
