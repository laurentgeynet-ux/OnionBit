// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';
import 'settings_defaults.dart';

/// Section « Bande passante » — limites globales de débit de la
/// session (`libtorrent/max_download_rate`, `max_upload_rate`,
/// octets/s — 0 = illimité). Saisie en Ko/s.
class BandwidthSection extends ConsumerStatefulWidget {
  const BandwidthSection({super.key});

  @override
  ConsumerState<BandwidthSection> createState() => _BandwidthSectionState();
}

class _BandwidthSectionState extends ConsumerState<BandwidthSection> {
  late final _deferred = DeferredSection(ref, 'bandwidth');
  final _down = TextEditingController();
  final _up = TextEditingController();
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
    _down.dispose();
    _up.dispose();
    super.dispose();
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    int kb(List<String> path) => settingsInt(settings, path) ~/ 1024;
    _down.text = '${kb(['libtorrent', 'max_download_rate'])}';
    _up.text = '${kb(['libtorrent', 'max_upload_rate'])}';
    _initialized = true;
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    int parseKb(String s) =>
        (int.tryParse(s.trim()) ?? 0).clamp(0, 1 << 40) * 1024;
    await applySettingsPatch(context, ref, {
      'libtorrent': {
        'max_download_rate': parseKb(_down.text),
        'max_upload_rate': parseKb(_up.text),
      },
    }, successMessage: context.l10n.bwSaved);
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.speed,
      title: l10n.sectionBandwidth,
      sectionId: 'bandwidth',
      defaults: kBandwidthDefaults,
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _RateControl(
              label: l10n.bwDownloadLabel,
              keyPath: const ['libtorrent', 'max_download_rate'],
              icon: Icons.arrow_downward,
              controller: _down,
              onChanged: _deferred.markDirty,
            ),
            const SizedBox(height: AppSpacing.sm),
            _RateControl(
              label: l10n.bwUploadLabel,
              keyPath: const ['libtorrent', 'max_upload_rate'],
              icon: Icons.arrow_upward,
              controller: _up,
              onChanged: _deferred.markDirty,
            ),
            const SizedBox(height: AppSpacing.md),
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

/// Ligne de limite de débit : champ numérique (Ko/s) + slider
/// exponentiel (0 = illimité ↔ 10 Mo/s) + chips de presets, tous
/// synchronisés. `0` = illimité, convention backend `0 → None`.
class _RateControl extends StatelessWidget {
  const _RateControl({
    required this.label,
    required this.keyPath,
    required this.icon,
    required this.controller,
    required this.onChanged,
  });

  final String label;
  final List<String> keyPath;
  final IconData icon;
  final TextEditingController controller;
  final VoidCallback onChanged;

  /// Plafond du slider : 10 Mo/s en Ko/s.
  static const _maxKb = 10 * 1024;

  /// Positions du slider : 0 = illimité, 1..100 = échelle
  /// exponentielle 1 Ko/s → 10 Mo/s (la granularité fine compte en
  /// bas de plage, pas en haut).
  static const _positions = 100.0;

  static double _toPos(int kb) =>
      kb <= 0 ? 0 : 1 + (log(kb) / log(_maxKb)) * (_positions - 1);

  static int _toKb(double pos) =>
      pos <= 0 ? 0 : exp((pos - 1) / (_positions - 1) * log(_maxKb)).round();

  int _kb() => (int.tryParse(controller.text.trim()) ?? 0).clamp(0, 1 << 40);

  void _setKb(int kb) {
    controller.text = '$kb';
    onChanged();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final presets = <(String, int)>[
      (l10n.rateUnlimited, 0),
      (l10n.ratePresetMb(1), 1024),
      (l10n.ratePresetMb(5), 5 * 1024),
      (l10n.ratePresetMb(10), _maxKb),
    ];
    final kb = _kb();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: controller,
                onChanged: (_) => onChanged(),
                keyboardType: TextInputType.number,
                decoration: InputDecoration(
                  labelText: l10n.rateFieldLabel(label),
                  hintText: l10n.rateUnlimitedHint,
                  prefixIcon: Icon(icon),
                  suffixIcon: KeyInfoIcon(
                    keyPath,
                    description: l10n.rateDesc,
                  ),
                ),
              ),
            ),
          ],
        ),
        Row(
          children: [
            Expanded(
              child: Slider(
                value: _toPos(kb).clamp(0, _positions),
                max: _positions,
                onChanged: (pos) => _setKb(_toKb(pos)),
              ),
            ),
            for (final (label, v) in presets)
              Padding(
                padding: const EdgeInsets.only(left: AppSpacing.xs),
                child: ChoiceChip(
                  label: Text(label),
                  selected: kb == v,
                  visualDensity: VisualDensity.compact,
                  onSelected: (_) => _setKb(v),
                ),
              ),
          ],
        ),
      ],
    );
  }
}
