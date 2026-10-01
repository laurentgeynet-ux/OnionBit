// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';
import 'settings_defaults.dart';

/// Section « File d'attente » — bornes du gestionnaire de file
/// (`libtorrent/active_*` : `-1` = illimité) et comportement
/// `auto_managed` par défaut des nouveaux téléchargements.
class QueueSection extends ConsumerStatefulWidget {
  const QueueSection({super.key});

  @override
  ConsumerState<QueueSection> createState() => _QueueSectionState();
}

class _QueueSectionState extends ConsumerState<QueueSection> {
  late final _deferred = DeferredSection(ref, 'queue');
  final _downloads = TextEditingController();
  final _seeds = TextEditingController();
  final _checking = TextEditingController();
  final _limit = TextEditingController();
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

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    setState(() => _saving = true);
    int parse(String s) => int.tryParse(s.trim()) ?? -1;
    await applySettingsPatch(context, ref, {
      'libtorrent': {
        'active_downloads': parse(_downloads.text),
        'active_seeds': parse(_seeds.text),
        'active_checking': parse(_checking.text),
        'active_limit': parse(_limit.text),
      },
    }, successMessage: 'File d\'attente enregistrée');
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  Widget _field(
    TextEditingController c,
    String label,
    int def,
    List<String> keyPath,
  ) => Expanded(
    child: TextField(
      controller: c,
      onChanged: (_) => _deferred.markDirty(),
      keyboardType: TextInputType.number,
      decoration: InputDecoration(
        labelText: label,
        hintText: 'défaut : $def · -1 = illimité',
        isDense: true,
        suffixIcon: KeyInfoIcon(keyPath),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) {
    return SettingsSection(
      icon: Icons.queue,
      title: 'File d\'attente',
      sectionId: 'queue',
      defaults: kQueueDefaults,
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Les téléchargements « gérés automatiquement » au-delà de '
              'ces bornes sont mis en pause puis repris quand un slot '
              'se libère.',
              style: Theme.of(context).textTheme.bodySmall
                  ?.copyWith(color: Theme.of(context).colorScheme.outline),
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                _field(_downloads, 'Téléchargements actifs', 3, const [
                  'libtorrent',
                  'active_downloads',
                ]),
                const SizedBox(width: AppSpacing.sm),
                _field(_seeds, 'Seeds actifs', 5, const [
                  'libtorrent',
                  'active_seeds',
                ]),
              ],
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                _field(_checking, 'Vérifications actives', 1, const [
                  'libtorrent',
                  'active_checking',
                ]),
                const SizedBox(width: AppSpacing.sm),
                _field(_limit, 'Limite globale', 500, const [
                  'libtorrent',
                  'active_limit',
                ]),
              ],
            ),
            SettingsSwitch(
              path: const ['libtorrent', 'fastresume_check'],
              value: settingsBool(settings, const [
                'libtorrent',
                'fastresume_check',
              ], def: true),
              title: 'Vérification au démarrage',
              subtitle:
                  'Relit un échantillon de pièces après la restauration '
                  'pour détecter les fichiers modifiés entre deux '
                  'sessions. Désactivé : aucune relecture, une '
                  'corruption passera inaperçue.',
            ),
            SettingsSwitch(
              path: const ['libtorrent', 'download_defaults', 'auto_managed'],
              value: settingsBool(settings, const [
                'libtorrent',
                'download_defaults',
                'auto_managed',
              ]),
              title: 'Gestion automatique par défaut',
              subtitle:
                  'Les nouveaux téléchargements sont placés sous la '
                  'file d\'attente. Défaut : désactivé.',
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
