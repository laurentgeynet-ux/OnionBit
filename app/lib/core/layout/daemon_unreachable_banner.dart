import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../config/connection_settings.dart';
import '../theme/app_theme.dart';

/// Bannière « daemon injoignable » — visible tant que le SSE est
/// coupé. « Configurer… » ouvre le dialogue de connexion (URL de base
/// + clé API) ; « Relancer la découverte » re-résout le daemon local.
class DaemonUnreachableBanner extends ConsumerWidget {
  const DaemonUnreachableBanner({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final scheme = Theme.of(context).colorScheme;
    final baseUrl = ref.watch(connectionSettingsProvider).value?.baseUrl ?? '';
    return Material(
      color: scheme.errorContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: AppSpacing.md,
          vertical: AppSpacing.xs,
        ),
        child: Row(
          children: [
            Icon(Icons.cloud_off, size: 18, color: scheme.onErrorContainer),
            const SizedBox(width: AppSpacing.sm),
            Expanded(
              child: Text(
                'Daemon injoignable${baseUrl.isNotEmpty ? ' — $baseUrl' : ''}',
                style: Theme.of(context).textTheme.bodySmall
                    ?.copyWith(color: scheme.onErrorContainer),
              ),
            ),
            TextButton.icon(
              onPressed: () =>
                  ref.read(connectionSettingsProvider.notifier).rediscover(),
              icon: const Icon(Icons.refresh, size: 16),
              label: const Text('Réessayer'),
            ),
            const SizedBox(width: AppSpacing.xs),
            FilledButton.tonalIcon(
              onPressed: () => _ConnectionDialog.show(context),
              icon: const Icon(Icons.settings_outlined, size: 16),
              label: const Text('Configurer…'),
            ),
          ],
        ),
      ),
    );
  }
}

/// Dialogue de première connexion : URL de base (`http://host:port`)
/// + clé API — même persistance que la section Réglages → Connexion.
class _ConnectionDialog extends ConsumerStatefulWidget {
  const _ConnectionDialog();

  static Future<void> show(BuildContext context) =>
      showDialog(context: context, builder: (_) => const _ConnectionDialog());

  @override
  ConsumerState<_ConnectionDialog> createState() => _ConnectionDialogState();
}

class _ConnectionDialogState extends ConsumerState<_ConnectionDialog> {
  final _url = TextEditingController();
  final _key = TextEditingController();
  bool _saving = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    final c = ref.read(connectionSettingsProvider).value;
    if (c != null) {
      _url.text = c.baseUrl;
      _key.text = c.apiKey;
    }
  }

  @override
  void dispose() {
    _url.dispose();
    _key.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final url = _url.text.trim();
    final uri = Uri.tryParse(url);
    if (uri == null || !uri.hasScheme || uri.host.isEmpty) {
      setState(() => _error = 'URL invalide (ex. http://127.0.0.1:8085)');
      return;
    }
    setState(() {
      _saving = true;
      _error = null;
    });
    await ref
        .read(connectionSettingsProvider.notifier)
        .save(baseUrl: url, apiKey: _key.text);
    // Re-résolution si loopback (daemon local relancé si mort).
    await ref.read(connectionSettingsProvider.notifier).rediscover();
    if (mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Connexion au daemon'),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: _url,
              decoration: InputDecoration(
                labelText: 'URL du daemon',
                hintText: 'http://127.0.0.1:8085',
                errorText: _error,
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: _key,
              obscureText: true,
              decoration: const InputDecoration(
                labelText: 'Clé API (si activée)',
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Annuler'),
        ),
        FilledButton(
          onPressed: _saving ? null : _save,
          child: Text(_saving ? 'Connexion…' : 'Enregistrer'),
        ),
      ],
    );
  }
}
