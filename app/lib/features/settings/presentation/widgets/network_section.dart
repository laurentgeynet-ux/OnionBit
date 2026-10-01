// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../../../diagnostic/presentation/providers/diagnostic_providers.dart';
import 'settings_section.dart';
import 'settings_defaults.dart';

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
  late final _deferred = DeferredSection(ref, 'network');

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
  void initState() {
    super.initState();
    _deferred.attach(save: _save, discard: _discard);
  }

  @override
  void dispose() {
    _deferred.detach();
    _proxyServer.dispose();
    _proxyAuth.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _proxyType = settingsInt(settings, const ['libtorrent', 'proxy_type']);
    if (!_proxyTypes.containsKey(_proxyType)) _proxyType = 0;
    _proxyServer.text = settingsString(settings, const [
      'libtorrent',
      'proxy_server',
    ]);
    _proxyAuth.text = settingsString(settings, const [
      'libtorrent',
      'proxy_auth',
    ]);
    _initialized = true;
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    setState(() => _saving = true);
    await applySettingsPatch(context, ref, {
      'libtorrent': {
        'proxy_type': _proxyType,
        'proxy_server': _proxyServer.text.trim(),
        'proxy_auth': _proxyAuth.text,
      },
    }, successMessage: 'Réglages réseau enregistrés');
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SettingsSection(
      icon: Icons.public,
      title: 'Réseau',
      sectionId: 'network',
      defaults: kNetworkDefaults,
      child: (context, settings) {
        _sync(settings);
        const lt = ['libtorrent'];
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _EffectiveState(settings: settings),
            const Divider(height: AppSpacing.lg),
            Text('Découverte & transport', style: theme.textTheme.labelMedium),
            SettingsSwitch(
              path: [...lt, 'dht'],
              value: settingsBool(settings, [...lt, 'dht'], def: true),
              title: 'DHT (mainline BEP 5)',
              subtitle: 'Défaut : activé. Pris en compte au redémarrage.',
            ),
            SettingsSwitch(
              path: [...lt, 'upnp'],
              value: settingsBool(settings, [...lt, 'upnp'], def: true),
              title: 'UPnP',
              subtitle: 'Défaut : activé. Pris en compte au redémarrage.',
            ),
            SettingsSwitch(
              path: [...lt, 'natpmp'],
              value: settingsBool(settings, [...lt, 'natpmp'], def: true),
              title: 'NAT-PMP',
              subtitle: 'Défaut : activé. Pris en compte au redémarrage.',
            ),
            SettingsSwitch(
              path: [...lt, 'lsd'],
              value: settingsBool(settings, [...lt, 'lsd'], def: true),
              title: 'Découverte locale (LSD)',
              subtitle: 'Défaut : activé. Pris en compte au redémarrage.',
            ),
            SettingsSwitch(
              path: [...lt, 'utp'],
              value: settingsBool(settings, [...lt, 'utp'], def: true),
              title: 'uTP',
              subtitle: 'Défaut : activé. Pris en compte au redémarrage.',
            ),
            const Divider(height: AppSpacing.lg),
            Text('Proxy sortant', style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpacing.xs),
            DropdownButtonFormField<int>(
              initialValue: _proxyType,
              decoration: const InputDecoration(
                labelText: 'Type de proxy (défaut : aucun)',
                suffixIcon: KeyInfoIcon(['libtorrent', 'proxy_type']),
              ),
              items: [
                for (final e in _proxyTypes.entries)
                  DropdownMenuItem(value: e.key, child: Text(e.value)),
              ],
              onChanged: (v) => setState(() {
                _proxyType = v ?? 0;
                _deferred.markDirty();
              }),
            ),
            if (_proxyType != 0) ...[
              const SizedBox(height: AppSpacing.sm),
              TextField(
                controller: _proxyServer,
                onChanged: (_) => _deferred.markDirty(),
                decoration: const InputDecoration(
                  labelText: 'Serveur (hôte:port)',
                  suffixIcon: KeyInfoIcon(['libtorrent', 'proxy_server']),
                  hintText: '127.0.0.1:9050',
                ),
              ),
              if (_proxyType == 3 || _proxyType == 5) ...[
                const SizedBox(height: AppSpacing.sm),
                TextField(
                  controller: _proxyAuth,
                  onChanged: (_) => _deferred.markDirty(),
                  obscureText: true,
                  decoration: const InputDecoration(
                    labelText: 'Authentification (utilisateur:mot de passe)',
                    suffixIcon: KeyInfoIcon([
                      'libtorrent',
                      'proxy_username',
                    ], description: 'Mot de passe : libtorrent/proxy_password'),
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

/// Carte « État effectif » — valeurs réellement appliquées par le
/// daemon (`listen_port`/`listen_interfaces` runtime, indicateurs de
/// découverte, proxy) + pairs TunnelCommunity en direct
/// (`/api/ipv8/overlays`). Lecture seule, distinguée des réglages
/// éditables : les commutateurs ci-dessous ne prennent effet qu'au
/// redémarrage.
class _EffectiveState extends ConsumerWidget {
  const _EffectiveState({required this.settings});

  final Map<String, dynamic> settings;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    const lt = ['libtorrent'];
    final port = settingsInt(settings, [...lt, 'listen_port']);
    final interfaces = settingsLeaf(settings, [...lt, 'listen_interfaces']);
    final proxyType = settingsInt(settings, [...lt, 'proxy_type']);
    final proxyServer = settingsString(settings, [...lt, 'proxy_server']);
    final overlays = ref.watch(overlaysProvider).value ?? const [];

    Widget flag(String label, bool on) => Chip(
      label: Text('$label : ${on ? 'actif' : 'inactif'}'),
      visualDensity: VisualDensity.compact,
      labelStyle: theme.textTheme.bodySmall,
    );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text('État effectif', style: theme.textTheme.labelMedium),
            const SizedBox(width: AppSpacing.xs),
            Tooltip(
              message:
                  'Valeurs réellement appliquées par le daemon '
                  '(lecture seule). Les réglages ci-dessous peuvent '
                  'différer tant que le daemon n\'a pas redémarré.',
              child: Icon(
                Icons.info_outline,
                size: 14,
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ),
        const SizedBox(height: AppSpacing.xs),
        Wrap(
          spacing: AppSpacing.xs,
          runSpacing: AppSpacing.xs,
          children: [
            Chip(
              label: Text('Port d\'écoute : ${port > 0 ? port : '—'}'),
              visualDensity: VisualDensity.compact,
              labelStyle: theme.textTheme.bodySmall,
            ),
            if (interfaces is List && interfaces.isNotEmpty)
              Chip(
                label: Text('Interfaces : ${interfaces.join(', ')}'),
                visualDensity: VisualDensity.compact,
                labelStyle: theme.textTheme.bodySmall,
              ),
            for (final e in const [
              ('DHT', 'dht'),
              ('UPnP', 'upnp'),
              ('NAT-PMP', 'natpmp'),
              ('LSD', 'lsd'),
              ('uTP', 'utp'),
            ])
              flag(e.$1, settingsBool(settings, [...lt, e.$2], def: true)),
            Chip(
              label: Text(
                proxyType == 0
                    ? 'Proxy : aucun'
                    : 'Proxy : type $proxyType'
                          '${proxyServer.isNotEmpty ? ' · $proxyServer' : ''}',
              ),
              visualDensity: VisualDensity.compact,
              labelStyle: theme.textTheme.bodySmall,
            ),
            for (final o in overlays)
              Chip(
                avatar: const Icon(Icons.hub, size: 14),
                label: Text('${o.name} : ${o.peers} pairs'),
                visualDensity: VisualDensity.compact,
                labelStyle: theme.textTheme.bodySmall,
              ),
          ],
        ),
      ],
    );
  }
}
