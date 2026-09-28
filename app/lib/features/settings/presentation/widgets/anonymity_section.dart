import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';

/// Section « Tunnels anonymes » — réglages de la `TunnelCommunity`
/// (`tunnel_community/*` : activation, bornes de circuits, rôle de
/// noeud de sortie).
class AnonymitySection extends ConsumerStatefulWidget {
  const AnonymitySection({super.key});

  @override
  ConsumerState<AnonymitySection> createState() => _AnonymitySectionState();
}

class _AnonymitySectionState extends ConsumerState<AnonymitySection> {
  static const _t = ['tunnel_community'];

  final _minCircuits = TextEditingController();
  final _maxCircuits = TextEditingController();
  bool _initialized = false;
  bool _saving = false;

  @override
  void dispose() {
    _minCircuits.dispose();
    _maxCircuits.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _minCircuits.text = '${settingsInt(settings, [..._t, 'min_circuits'], def: 3)}';
    _maxCircuits.text = '${settingsInt(settings, [..._t, 'max_circuits'], def: 8)}';
    _initialized = true;
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    final min = int.tryParse(_minCircuits.text.trim()) ?? 3;
    final max = int.tryParse(_maxCircuits.text.trim()) ?? 8;
    if (min < 0 || max < min) {
      setState(() => _saving = false);
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('min_circuits doit être ≤ max_circuits')),
      );
      return;
    }
    await applySettingsPatch(
      context,
      ref,
      {
        'tunnel_community': {'min_circuits': min, 'max_circuits': max},
      },
      successMessage: 'Réglages des tunnels enregistrés',
    );
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SettingsSection(
      icon: Icons.shield_outlined,
      title: 'Tunnels anonymes',
      child: (context, settings) {
        _sync(settings);
        final enabled = settingsBool(settings, [..._t, 'enabled'], def: true);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SettingsSwitch(
              path: [..._t, 'enabled'],
              value: enabled,
              title: 'TunnelCommunity activée',
              subtitle:
                  'Requis pour les téléchargements anonymes. '
                  'Pris en compte au redémarrage.',
            ),
            SettingsSwitch(
              path: [..._t, 'exitnode_enabled'],
              value: settingsBool(settings, [..._t, 'exitnode_enabled']),
              title: 'Agir comme noeud de sortie',
              subtitle:
                  'Attention : votre machine relaye alors le trafic '
                  'BitTorrent des autres pairs vers l\'Internet public.',
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _minCircuits,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                      labelText: 'Circuits minimum',
                      isDense: true,
                    ),
                  ),
                ),
                const SizedBox(width: AppSpacing.sm),
                Expanded(
                  child: TextField(
                    controller: _maxCircuits,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                      labelText: 'Circuits maximum',
                      isDense: true,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: AppSpacing.xs),
            Text(
              'Circuits maintenus en permanence par lane anonyme ; '
              'le détail en direct est visible dans Diagnostic → Circuits.',
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
                label: const Text('Enregistrer'),
              ),
            ),
          ],
        );
      },
    );
  }
}
