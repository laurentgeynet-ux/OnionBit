import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import 'settings_section.dart';

/// Section « Réseau » — transports et découverte de la session
/// (`libtorrent/{dht,upnp,natpmp,lsd,utp}`) et proxy sortant
/// (`proxy_type` enum libtorrent, `proxy_server` host:port,
/// `proxy_auth` user:pass).
class NetworkSection extends ConsumerStatefulWidget {
  const NetworkSection({super.key});

  @override
  ConsumerState<NetworkSection> createState() => _NetworkSectionState();
}

class _NetworkSectionState extends ConsumerState<NetworkSection> {
  /// Valeurs `proxy_type` de `lt::settings_pack` (enum Python).
  static const _proxyTypes = {
    0: 'Aucun',
    2: 'SOCKS5',
    3: 'SOCKS5 + authentification',
    4: 'HTTP',
    5: 'HTTP + authentification',
  };

  int _proxyType = 0;
  final _proxyServer = TextEditingController();
  final _proxyAuth = TextEditingController();
  bool _initialized = false;
  bool _saving = false;

  @override
  void dispose() {
    _proxyServer.dispose();
    _proxyAuth.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _proxyType = settingsInt(settings, const ['libtorrent', 'proxy_type']);
    if (!_proxyTypes.containsKey(_proxyType)) _proxyType = 0;
    _proxyServer.text = settingsString(
      settings,
      const ['libtorrent', 'proxy_server'],
    );
    _proxyAuth.text = settingsString(
      settings,
      const ['libtorrent', 'proxy_auth'],
    );
    _initialized = true;
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    await applySettingsPatch(
      context,
      ref,
      {
        'libtorrent': {
          'proxy_type': _proxyType,
          'proxy_server': _proxyServer.text.trim(),
          'proxy_auth': _proxyAuth.text,
        },
      },
      successMessage: 'Réglages réseau enregistrés',
    );
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SettingsSection(
      icon: Icons.public,
      title: 'Réseau',
      child: (context, settings) {
        _sync(settings);
        const lt = ['libtorrent'];
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Découverte & transport', style: theme.textTheme.labelMedium),
            SettingsSwitch(
              path: [...lt, 'dht'],
              value: settingsBool(settings, [...lt, 'dht'], def: true),
              title: 'DHT (mainline BEP 5)',
            ),
            SettingsSwitch(
              path: [...lt, 'upnp'],
              value: settingsBool(settings, [...lt, 'upnp'], def: true),
              title: 'UPnP',
            ),
            SettingsSwitch(
              path: [...lt, 'natpmp'],
              value: settingsBool(settings, [...lt, 'natpmp'], def: true),
              title: 'NAT-PMP',
            ),
            SettingsSwitch(
              path: [...lt, 'lsd'],
              value: settingsBool(settings, [...lt, 'lsd'], def: true),
              title: 'Découverte locale (LSD)',
            ),
            SettingsSwitch(
              path: [...lt, 'utp'],
              value: settingsBool(settings, [...lt, 'utp'], def: true),
              title: 'uTP',
            ),
            const Divider(height: AppSpacing.lg),
            Text('Proxy sortant', style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpacing.xs),
            DropdownButtonFormField<int>(
              initialValue: _proxyType,
              decoration: const InputDecoration(labelText: 'Type de proxy'),
              items: [
                for (final e in _proxyTypes.entries)
                  DropdownMenuItem(value: e.key, child: Text(e.value)),
              ],
              onChanged: (v) => setState(() => _proxyType = v ?? 0),
            ),
            if (_proxyType != 0) ...[
              const SizedBox(height: AppSpacing.sm),
              TextField(
                controller: _proxyServer,
                decoration: const InputDecoration(
                  labelText: 'Serveur (hôte:port)',
                  hintText: '127.0.0.1:9050',
                ),
              ),
              if (_proxyType == 3 || _proxyType == 5) ...[
                const SizedBox(height: AppSpacing.sm),
                TextField(
                  controller: _proxyAuth,
                  obscureText: true,
                  decoration: const InputDecoration(
                    labelText: 'Authentification (utilisateur:mot de passe)',
                  ),
                ),
              ],
            ],
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
