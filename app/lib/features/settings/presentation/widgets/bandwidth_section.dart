// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
import 'settings_section.dart';
import 'settings_defaults.dart';

/// Section « Bande passante » — limites globales de débit de la
/// session (`libtorrent/max_download_rate`, `max_upload_rate`,
/// octets/s — 0 = illimité). Saisie en Mo/s (décimales admises).
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
    _down.text = _fmtMb(
      settingsInt(settings, const ['libtorrent', 'max_download_rate']),
    );
    _up.text = _fmtMb(
      settingsInt(settings, const ['libtorrent', 'max_upload_rate']),
    );
    _initialized = true;
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    await applySettingsPatch(context, ref, {
      'libtorrent': {
        'max_download_rate': _parseMb(_down.text),
        'max_upload_rate': _parseMb(_up.text),
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
            const SizedBox(height: AppSpace.sm),
            _RateControl(
              label: l10n.bwUploadLabel,
              keyPath: const ['libtorrent', 'max_upload_rate'],
              icon: Icons.arrow_upward,
              controller: _up,
              onChanged: _deferred.markDirty,
            ),
            const SizedBox(height: AppSpace.md),
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

/// Octets/s → texte de saisie en Mo/s : entiers sans décimale,
/// sinon 2 chiffres max (`0.5`, `1.25`).
String _fmtMb(int bytesPerSec) {
  final mb = bytesPerSec / 1048576;
  return mb == mb.truncateToDouble()
      ? '${mb.truncate()}'
      : mb.toStringAsFixed(2).replaceFirst(RegExp(r'\.?0+$'), '');
}

/// Saisie Mo/s → octets/s (`0` ou invalide → 0 = illimité ; la
/// virgule décimale est acceptée).
int _parseMb(String s) =>
    (((double.tryParse(s.trim().replaceAll(',', '.')) ?? 0).clamp(
              0.0,
              1 << 40,
            )) *
            1048576)
        .round();

/// Ligne de limite de débit : champ numérique (Mo/s) + slider
/// exponentiel (0 = illimité ↔ 10 Mo/s) + chip « Illimité », tous
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

  /// Champ saisi en Mo/s → valeur interne en Ko/s pour le slider.
  int _kb() => (_parseMb(controller.text) / 1024).round();

  void _setKb(int kb) {
    controller.text = _fmtMb(kb * 1024);
    onChanged();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
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
                keyboardType: const TextInputType.numberWithOptions(
                  decimal: true,
                ),
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
            Padding(
              padding: const EdgeInsets.only(left: AppSpace.xs),
              child: ChoiceChip(
                label: Text(l10n.rateUnlimited),
                selected: kb == 0,
                visualDensity: VisualDensity.compact,
                onSelected: (_) => _setKb(0),
              ),
            ),
          ],
        ),
      ],
    );
  }
}
