// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';

/// Section « Avancé » — éditeur brut de l'arbre `configuration.json`
/// retourné par `GET /api/settings`. Fallback pour les réglages non
/// exposés dans les sections dédiées : le JSON validé est envoyé tel
/// quel au endpoint merge (`POST /api/settings`).
class AdvancedSection extends ConsumerStatefulWidget {
  const AdvancedSection({super.key});

  @override
  ConsumerState<AdvancedSection> createState() => _AdvancedSectionState();
}

class _AdvancedSectionState extends ConsumerState<AdvancedSection> {
  final _controller = TextEditingController();
  late final _deferred = DeferredSection(ref, 'advanced');
  bool _initialized = false;
  bool _saving = false;
  String? _parseError;

  @override
  void initState() {
    super.initState();
    _deferred.attach(save: _save, discard: _discard);
  }

  @override
  void dispose() {
    _deferred.detach();
    _controller.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _controller.text = const JsonEncoder.withIndent('  ').convert(settings);
    _initialized = true;
  }

  void _discard() => setState(() {
    _initialized = false;
    _parseError = null;
    _deferred.markClean();
  });

  Map<String, dynamic>? _parse() {
    try {
      final decoded = jsonDecode(_controller.text);
      if (decoded is! Map<String, dynamic>) {
        setState(() => _parseError = 'Le document doit être un objet JSON.');
        return null;
      }
      setState(() => _parseError = null);
      return decoded;
    } on FormatException catch (e) {
      setState(() => _parseError = 'JSON invalide : ${e.message}');
      return null;
    }
  }

  Future<void> _save() async {
    final patch = _parse();
    if (patch == null) return;
    setState(() => _saving = true);
    await applySettingsPatch(
      context,
      ref,
      patch,
      successMessage: 'Configuration avancée appliquée',
    );
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  Future<void> _copyAll() async {
    await Clipboard.setData(ClipboardData(text: _controller.text));
    if (mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('Configuration copiée')));
    }
  }

  Future<void> _import() async {
    final json = await showDialog<String>(
      context: context,
      builder: (context) => const _ImportDialog(),
    );
    if (json == null || !mounted) return;
    setState(() {
      _controller.text = json;
      _deferred.markDirty();
      _parseError = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SettingsSection(
      icon: Icons.data_object,
      title: 'Configuration avancée',
      sectionId: 'advanced',
      child: (context, settings) {
        _sync(settings);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.warning_amber,
                  size: 18,
                  color: theme.colorScheme.tertiary,
                ),
                const SizedBox(width: AppSpacing.xs),
                Expanded(
                  child: Text(
                    'Édition brute de l\'arbre `configuration.json`. '
                    'Les clés sensibles (clés API, chemins d\'identité) '
                    'sont visibles — à manipuler avec précaution.',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.tertiary,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: _controller,
              maxLines: 18,
              style: theme.textTheme.bodySmall?.copyWith(
                fontFamily: 'monospace',
              ),
              decoration: InputDecoration(
                isDense: true,
                border: const OutlineInputBorder(),
                errorText: _parseError,
                errorMaxLines: 3,
              ),
              onChanged: (_) => _deferred.markDirty(),
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton.icon(
                  onPressed: _copyAll,
                  icon: const Icon(Icons.copy, size: 18),
                  label: const Text('Exporter'),
                ),
                TextButton.icon(
                  onPressed: _import,
                  icon: const Icon(Icons.file_upload_outlined, size: 18),
                  label: const Text('Importer'),
                ),
                const Spacer(),
                TextButton.icon(
                  onPressed: _discard,
                  icon: const Icon(Icons.refresh, size: 18),
                  label: const Text('Recharger'),
                ),
                const SizedBox(width: AppSpacing.xs),
                FilledButton.icon(
                  onPressed: _saving ? null : _save,
                  icon: _saving
                      ? const SizedBox(
                          width: 14,
                          height: 14,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.save, size: 18),
                  label: const Text('Appliquer'),
                ),
              ],
            ),
          ],
        );
      },
    );
  }
}

/// Dialogue d'import : colle un JSON de configuration, valide la forme
/// et affiche un aperçu des sections racines avant d'injecter le texte
/// dans l'éditeur (l'application reste soumise à « Appliquer »).
class _ImportDialog extends StatefulWidget {
  const _ImportDialog();

  @override
  State<_ImportDialog> createState() => _ImportDialogState();
}

class _ImportDialogState extends State<_ImportDialog> {
  final _controller = TextEditingController();
  String? _error;
  List<String> _sections = const [];

  void _validate(String text) {
    setState(() {
      try {
        final decoded = jsonDecode(text);
        if (decoded is! Map<String, dynamic>) {
          _error = 'Le document doit être un objet JSON.';
          _sections = const [];
          return;
        }
        _error = null;
        _sections = decoded.keys.toList()..sort();
      } on FormatException catch (e) {
        _error = 'JSON invalide : ${e.message}';
        _sections = const [];
      }
    });
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Importer une configuration'),
      content: SizedBox(
        width: 480,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: _controller,
              maxLines: 10,
              style: Theme.of(context).textTheme.bodySmall
                  ?.copyWith(fontFamily: 'monospace'),
              decoration: InputDecoration(
                hintText: '{ "libtorrent": { ... }, ... }',
                border: const OutlineInputBorder(),
                errorText: _error,
                errorMaxLines: 3,
              ),
              onChanged: _validate,
            ),
            if (_sections.isNotEmpty) ...[
              const SizedBox(height: AppSpacing.sm),
              Text(
                'Sections détectées : ${_sections.join(', ')}',
                style: Theme.of(context).textTheme.bodySmall,
              ),
              Text(
                'Le contenu remplacera l\'éditeur — rien n\'est envoyé '
                'tant que « Appliquer » n\'est pas pressé.',
                style: Theme.of(context).textTheme.bodySmall
                    ?.copyWith(color: Theme.of(context).colorScheme.outline),
              ),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Annuler'),
        ),
        FilledButton(
          onPressed: _error == null && _sections.isNotEmpty
              ? () => Navigator.of(context).pop(_controller.text)
              : null,
          child: const Text('Charger dans l\'éditeur'),
        ),
      ],
    );
  }
}
