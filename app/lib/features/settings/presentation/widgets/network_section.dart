// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../l10n/app_localizations.dart' show AppLocalizations;
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

  /// Valeurs `proxy_type` de `lt::settings_pack` (enum Python) —
  /// libellés localisés via [_proxyTypeLabel].
  static const _proxyTypeValues = [0, 2, 3, 4, 5];

  /// Libellé localisé d'un `proxy_type` libtorrent.
  static String _proxyTypeLabel(AppLocalizations l10n, int type) =>
      switch (type) {
        0 => l10n.proxyNone,
        2 => 'SOCKS5',
        3 => l10n.proxySocks5Auth,
        4 => 'HTTP',
        5 => l10n.proxyHttpAuth,
        _ => '$type',
      };

  int _proxyType = 0;
  final _port = TextEditingController();
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
    _port.dispose();
    _proxyServer.dispose();
    _proxyAuth.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _port.text = '${settingsInt(settings, const ['libtorrent', 'port'])}';
    _proxyType = settingsInt(settings, const ['libtorrent', 'proxy_type']);
    if (!_proxyTypeValues.contains(_proxyType)) _proxyType = 0;
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
        'port': (int.tryParse(_port.text.trim()) ?? 0).clamp(0, 65535),
        'proxy_type': _proxyType,
        'proxy_server': _proxyServer.text.trim(),
        'proxy_auth': _proxyAuth.text,
      },
    }, successMessage: context.l10n.netSaved);
    _deferred.markClean();
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    return SettingsSection(
      icon: Icons.public,
      title: l10n.sectionNetwork,
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
            Text(l10n.netDiscovery, style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpacing.sm),
            SizedBox(
              width: 220,
              child: TextField(
                controller: _port,
                onChanged: (_) => _deferred.markDirty(),
                keyboardType: TextInputType.number,
                decoration: InputDecoration(
                  labelText: l10n.netListenPort,
                  helperText: l10n.netListenPortHint,
                  helperMaxLines: 2,
                  suffixIcon: const KeyInfoIcon(['libtorrent', 'port']),
                ),
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            SettingsSwitch(
              path: [...lt, 'dht'],
              value: settingsBool(settings, [...lt, 'dht'], def: true),
              title: 'DHT (mainline BEP 5)',
              subtitle: l10n.appliedOnRestartOn,
            ),
            SettingsSwitch(
              path: [...lt, 'upnp'],
              value: settingsBool(settings, [...lt, 'upnp'], def: true),
              title: 'UPnP',
              subtitle: l10n.appliedOnRestartOn,
            ),
            SettingsSwitch(
              path: [...lt, 'natpmp'],
              value: settingsBool(settings, [...lt, 'natpmp'], def: true),
              title: 'NAT-PMP',
              subtitle: l10n.appliedOnRestartOn,
            ),
            SettingsSwitch(
              path: [...lt, 'lsd'],
              value: settingsBool(settings, [...lt, 'lsd'], def: true),
              title: l10n.lsdTitle,
              subtitle: l10n.appliedOnRestartOn,
            ),
            SettingsSwitch(
              path: [...lt, 'utp'],
              value: settingsBool(settings, [...lt, 'utp'], def: true),
              title: 'uTP',
              subtitle: l10n.appliedOnRestartOn,
            ),
            const Divider(height: AppSpacing.lg),
            Text(l10n.proxySection, style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpacing.xs),
            DropdownButtonFormField<int>(
              initialValue: _proxyType,
              decoration: InputDecoration(
                labelText: l10n.proxyTypeLabel,
                suffixIcon: const KeyInfoIcon(['libtorrent', 'proxy_type']),
              ),
              items: [
                for (final t in _proxyTypeValues)
                  DropdownMenuItem(
                    value: t,
                    child: Text(_proxyTypeLabel(l10n, t)),
                  ),
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
                decoration: InputDecoration(
                  labelText: l10n.proxyServerLabel,
                  suffixIcon: const KeyInfoIcon(['libtorrent', 'proxy_server']),
                  hintText: '127.0.0.1:9050',
                ),
              ),
              if (_proxyType == 3 || _proxyType == 5) ...[
                const SizedBox(height: AppSpacing.sm),
                TextField(
                  controller: _proxyAuth,
                  onChanged: (_) => _deferred.markDirty(),
                  obscureText: true,
                  decoration: InputDecoration(
                    labelText: l10n.proxyAuthLabel,
                    suffixIcon: KeyInfoIcon(
                      const ['libtorrent', 'proxy_username'],
                      description: l10n.proxyAuthDesc,
                    ),
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
                label: Text(l10n.save),
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
    final l10n = context.l10n;
    const lt = ['libtorrent'];
    final port = settingsInt(settings, [...lt, 'port']);
    final listenV4 = settingsString(settings, [...lt, 'listen_interface']);
    final listenV6 = settingsString(settings, [...lt, 'listen_interface_v6']);
    final interfaces = [
      if (listenV4.isNotEmpty) '$listenV4:$port',
      if (listenV6.isNotEmpty) '[$listenV6]:${settingsInt(settings, [...lt, 'port_v6'])}',
    ];
    final proxyType = settingsInt(settings, [...lt, 'proxy_type']);
    final proxyServer = settingsString(settings, [...lt, 'proxy_server']);
    final overlays = ref.watch(overlaysProvider).value ?? const [];

    Widget flag(String label, bool on) => Chip(
      label: Text('$label : ${on ? l10n.stateOn : l10n.stateOff}'),
      visualDensity: VisualDensity.compact,
      labelStyle: theme.textTheme.bodySmall,
    );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text(l10n.effectiveState, style: theme.textTheme.labelMedium),
            const SizedBox(width: AppSpacing.xs),
            Tooltip(
              message: l10n.effectiveStateTip,
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
              label: Text(
                l10n.listenPortChip(port > 0 ? '$port' : '—'),
              ),
              visualDensity: VisualDensity.compact,
              labelStyle: theme.textTheme.bodySmall,
            ),
            if (interfaces.isNotEmpty)
              Chip(
                label: Text(l10n.interfacesChip(interfaces.join(', '))),
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
                    ? l10n.proxyChipNone
                    : l10n.proxyChipType(
                        proxyType,
                        proxyServer.isNotEmpty ? ' · $proxyServer' : '',
                      ),
              ),
              visualDensity: VisualDensity.compact,
              labelStyle: theme.textTheme.bodySmall,
            ),
            for (final o in overlays)
              Chip(
                avatar: const Icon(Icons.hub, size: 14),
                label: Text(l10n.overlayPeersChip(o.name, o.peers)),
                visualDensity: VisualDensity.compact,
                labelStyle: theme.textTheme.bodySmall,
              ),
          ],
        ),
      ],
    );
  }
}
