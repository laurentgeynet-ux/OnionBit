import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';

/// Section « Bande passante » — limites globales de débit de la
/// session (`libtorrent/max_download_rate`, `max_upload_rate`,
/// octets/s — 0 = illimité). Saisie en Ko/s.
class BandwidthSection extends ConsumerStatefulWidget {
  const BandwidthSection({super.key});

  @override
  ConsumerState<BandwidthSection> createState() => _BandwidthSectionState();
}

class _BandwidthSectionState extends ConsumerState<BandwidthSection> {
  final _down = TextEditingController();
  final _up = TextEditingController();
  bool _initialized = false;
  bool _saving = false;

  @override
  void dispose() {
    _down.dispose();
    _up.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    int kb(List<String> path) => settingsInt(settings, path) ~/ 1024;
    _down.text = '${kb(['libtorrent', 'max_download_rate'])}';
    _up.text = '${kb(['libtorrent', 'max_upload_rate'])}';
    _initialized = true;
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    int parseKb(String s) => (int.tryParse(s.trim()) ?? 0).clamp(0, 1 << 40) * 1024;
    await applySettingsPatch(
      context,
      ref,
      {
        'libtorrent': {
          'max_download_rate': parseKb(_down.text),
          'max_upload_rate': parseKb(_up.text),
        },
      },
      successMessage: 'Limites de bande passante enregistrées',
    );
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    return SettingsSection(
      icon: Icons.speed,
      title: 'Bande passante',
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _down,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                      labelText: 'Téléchargement (Ko/s)',
                      hintText: 'défaut : 0 = illimité',
                      prefixIcon: Icon(Icons.arrow_downward),
                    ),
                  ),
                ),
                const SizedBox(width: AppSpacing.md),
                Expanded(
                  child: TextField(
                    controller: _up,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                      labelText: 'Envoi (Ko/s)',
                      hintText: 'défaut : 0 = illimité',
                      prefixIcon: Icon(Icons.arrow_upward),
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: AppSpacing.md),
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
