// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../providers/diagnostic_providers.dart';

/// Dialogue « Publier une attestation » (`POST /api/ipv8/ext/attest`,
/// ADR-0015 §6) — `kind` : infohash|channel, `subject` : hex (40 ou
/// 128 chars), `verdict` : endorse|flag. Rend `true` en succès.
/// `initialSubject`/`initialVerdict` pré-remplissent le formulaire
/// (appel depuis le menu contextuel d'un résultat de recherche).
class AttestDialog extends ConsumerStatefulWidget {
  const AttestDialog({super.key, this.initialSubject, this.initialVerdict});

  /// Sujet hex pré-rempli (ex. infohash d'un torrent).
  final String? initialSubject;

  /// Verdict pré-sélectionné (`endorse` ou `flag`).
  final String? initialVerdict;

  /// Affiche le dialogue et publie ; rend `true` en succès.
  static Future<bool> show(
    BuildContext context, {
    String? subject,
    String? verdict,
  }) async =>
      await showDialog<bool>(
        context: context,
        builder: (_) =>
            AttestDialog(initialSubject: subject, initialVerdict: verdict),
      ) ==
      true;

  @override
  ConsumerState<AttestDialog> createState() => _AttestDialogState();
}

class _AttestDialogState extends ConsumerState<AttestDialog> {
  late final TextEditingController _subject;
  String _kind = 'infohash';
  String _verdict = 'endorse';
  bool _sending = false;

  @override
  void initState() {
    super.initState();
    _subject = TextEditingController(text: widget.initialSubject ?? '');
    if (widget.initialVerdict == 'flag') _verdict = 'flag';
  }

  @override
  void dispose() {
    _subject.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    final subject = _subject.text.trim();
    final hexOk = RegExp(r'^[0-9a-fA-F]{40}$|^[0-9a-fA-F]{128}$')
        .hasMatch(subject);
    if (!hexOk) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(context.l10n.extAttestInvalid)));
      return;
    }
    setState(() => _sending = true);
    try {
      await ref
          .read(diagnosticRepositoryProvider)
          .extAttest(kind: _kind, subject: subject, verdict: _verdict);
      if (mounted) Navigator.of(context).pop(true);
    } catch (e) {
      if (mounted) {
        setState(() => _sending = false);
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(context.l10n.errorMessage('$e'))),
        );
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return AlertDialog(
      title: Text(l10n.extPublishAttest),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          DropdownButtonFormField<String>(
            initialValue: _kind,
            decoration: InputDecoration(labelText: l10n.extAttestKind),
            items: [
              DropdownMenuItem(value: 'infohash', child: Text('infohash')),
              DropdownMenuItem(value: 'channel', child: Text('channel')),
            ],
            onChanged: (v) => setState(() => _kind = v ?? 'infohash'),
          ),
          TextField(
            controller: _subject,
            decoration: InputDecoration(
              labelText: l10n.extAttestSubject,
              helperText: l10n.extAttestSubjectHint,
            ),
            style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
          ),
          const SizedBox(height: AppSpacing.sm),
          SegmentedButton<String>(
            segments: [
              ButtonSegment(
                value: 'endorse',
                icon: const Icon(Icons.thumb_up_outlined, size: 16),
                label: Text(l10n.extAttestEndorse),
              ),
              ButtonSegment(
                value: 'flag',
                icon: const Icon(Icons.flag_outlined, size: 16),
                label: Text(l10n.extAttestFlag),
              ),
            ],
            selected: {_verdict},
            onSelectionChanged: (s) => setState(() => _verdict = s.first),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: _sending ? null : () => Navigator.of(context).pop(false),
          child: Text(l10n.cancel),
        ),
        FilledButton(
          onPressed: _sending ? null : _send,
          child: Text(l10n.extPublishAttest),
        ),
      ],
    );
  }
}
