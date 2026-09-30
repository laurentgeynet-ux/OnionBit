import 'dart:convert';

import 'package:flutter/material.dart';
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
