// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
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
    }, successMessage: context.l10n.queueSaved);
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
        hintText: context.l10n.queueDefHint(def),
        isDense: true,
        suffixIcon: KeyInfoIcon(keyPath),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.queue,
      title: l10n.sectionQueue,
      sectionId: 'queue',
      defaults: kQueueDefaults,
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              l10n.queueHelp,
              style: Theme.of(context).textTheme.bodySmall
                  ?.copyWith(color: Theme.of(context).colorScheme.outline),
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                _field(_downloads, l10n.queueActiveDownloads, 3, const [
                  'libtorrent',
                  'active_downloads',
                ]),
                const SizedBox(width: AppSpacing.sm),
                _field(_seeds, l10n.queueActiveSeeds, 5, const [
                  'libtorrent',
                  'active_seeds',
                ]),
              ],
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                _field(_checking, l10n.queueActiveChecking, 1, const [
                  'libtorrent',
                  'active_checking',
                ]),
                const SizedBox(width: AppSpacing.sm),
                _field(_limit, l10n.queueActiveLimit, 500, const [
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
              title: l10n.queueFastresume,
              subtitle: l10n.queueFastresumeSub,
            ),
            SettingsSwitch(
              path: const ['libtorrent', 'download_defaults', 'auto_managed'],
              value: settingsBool(settings, const [
                'libtorrent',
                'download_defaults',
                'auto_managed',
              ]),
              title: l10n.queueAutoManaged,
              subtitle: l10n.queueAutoManagedSub,
            ),
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
