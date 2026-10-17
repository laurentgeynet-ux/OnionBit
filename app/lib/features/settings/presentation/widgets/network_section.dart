// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/design/design_tokens.dart';
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
  String _torMode = 'embedded';
  final _port = TextEditingController();
  final _proxyServer = TextEditingController();
  final _proxyAuth = TextEditingController();
  final _torSocks = TextEditingController();
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
    _torSocks.dispose();
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
    const tor = ['tor'];
    _torMode = settingsString(settings, [...tor, 'mode']);
    if (_torMode != 'embedded' && _torMode != 'external') {
      _torMode = 'embedded';
    }
    _torSocks.text = settingsString(settings, [...tor, 'external_socks5']);
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
      'tor': {
        'mode': _torMode,
        'external_socks5': _torSocks.text.trim(),
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
            const Divider(height: AppSpace.lg),
            Text(l10n.netDiscovery, style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpace.sm),
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
            const SizedBox(height: AppSpace.sm),
            SettingsSwitch(
              path: [...lt, 'dht'],
              value: settingsBool(settings, [...lt, 'dht'], def: true),
              title: 'DHT (mainline BEP 5)',
              restartOnly: true,
              defaultValue: true,
            ),
            SettingsSwitch(
              path: [...lt, 'upnp'],
              value: settingsBool(settings, [...lt, 'upnp'], def: true),
              title: 'UPnP',
              restartOnly: true,
              defaultValue: true,
            ),
            SettingsSwitch(
              path: [...lt, 'natpmp'],
              value: settingsBool(settings, [...lt, 'natpmp'], def: true),
              title: 'NAT-PMP',
              restartOnly: true,
              defaultValue: true,
            ),
            SettingsSwitch(
              path: [...lt, 'lsd'],
              value: settingsBool(settings, [...lt, 'lsd'], def: true),
              title: l10n.lsdTitle,
              restartOnly: true,
              defaultValue: true,
            ),
            SettingsSwitch(
              path: [...lt, 'utp'],
              value: settingsBool(settings, [...lt, 'utp'], def: true),
              title: 'uTP',
              restartOnly: true,
              defaultValue: true,
            ),
            const Divider(height: AppSpace.lg),
            Text(l10n.proxySection, style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpace.xs),
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
              const SizedBox(height: AppSpace.sm),
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
                const SizedBox(height: AppSpace.sm),
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
            const Divider(height: AppSpace.lg),
            // Connectivité Tor (ADR-0028 §8) — lane permanente activée
            // par défaut ; le toggle est immédiat comme les autres
            // commutateurs (hot-swap du runtime, sans redémarrage),
            // mode/endpoint partent avec le lot différé et s'appliquent
            // à l'enregistrement.
            Text(l10n.torSettingsTitle, style: theme.textTheme.labelMedium),
            const SizedBox(height: AppSpace.xs),
            SettingsSwitch(
              path: const ['tor', 'enabled'],
              value: settingsBool(settings, const ['tor', 'enabled']),
              title: l10n.torEnabled,
              subtitle: l10n.torEnabledHint,
              defaultValue: true,
            ),
            const SizedBox(height: AppSpace.xs),
            SizedBox(
              width: 320,
              child: DropdownButtonFormField<String>(
                initialValue: _torMode,
                isExpanded: true,
                decoration: InputDecoration(
                  labelText: l10n.torMode,
                  suffixIcon: const KeyInfoIcon(['tor', 'mode']),
                ),
                items: [
                  DropdownMenuItem(
                    value: 'embedded',
                    child: Text(l10n.torModeEmbedded),
                  ),
                  DropdownMenuItem(
                    value: 'external',
                    child: Text(l10n.torModeExternal),
                  ),
                ],
                onChanged: (v) => setState(() {
                  _torMode = v ?? 'embedded';
                  _deferred.markDirty();
                }),
              ),
            ),
            if (_torMode == 'external') ...[
              const SizedBox(height: AppSpace.sm),
              SizedBox(
                width: 320,
                child: TextField(
                  controller: _torSocks,
                  onChanged: (_) => _deferred.markDirty(),
                  decoration: InputDecoration(
                    labelText: l10n.torSocks5,
                    helperText: l10n.torSocks5Hint,
                    suffixIcon: const KeyInfoIcon(
                      ['tor', 'external_socks5'],
                    ),
                    hintText: 'socks5h://127.0.0.1:9050',
                  ),
                ),
              ),
            ],
            const SizedBox(height: AppSpace.xs),
            _TorAllowlist(settings: settings),
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
            const SizedBox(width: AppSpace.xs),
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
        const SizedBox(height: AppSpace.xs),
        Wrap(
          spacing: AppSpace.xs,
          runSpacing: AppSpace.xs,
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

/// Allowlist Tor en lecture seule (ADR-0028 §5) : politique
/// **effective** de `TorTargetPolicy` = `tor.allowed_hosts` ∪
/// `tor.search.backends.<id>.mirrors` (le miroir TPB est allowlisté
/// implicitement — le montrer évite une frontière invisible).
/// Non éditable ici — la liste est une frontière de sécurité définie
/// dans `configuration.json`.
class _TorAllowlist extends StatelessWidget {
  const _TorAllowlist({required this.settings});

  final Map<String, dynamic> settings;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final tor = settings['tor'];
    final search = tor is Map ? tor['search'] : null;
    final backendsMap =
        search is Map && search['backends'] is Map
            ? search['backends'] as Map
            : const {};
    // Union des hôtes : allowed_hosts + miroirs de chaque backend
    // (ordre stable, dédupliqué).
    final hosts = <String>{
      if (tor is Map)
        for (final h in (tor['allowed_hosts'] as List?) ?? const []) '$h',
      for (final b in backendsMap.values)
        if (b is Map)
          for (final m in (b['mirrors'] as List?) ?? const []) '$m',
    }.toList()..sort();
    final backends = backendsMap.keys.map((k) => '$k').toList()..sort();
    if (hosts.isEmpty && backends.isEmpty) return const SizedBox.shrink();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: AppSpace.sm),
        if (hosts.isNotEmpty) ...[
          Text(
            l10n.torAllowedHosts,
            style: theme.textTheme.bodySmall,
          ),
          const SizedBox(height: AppSpace.xs),
          Wrap(
            spacing: AppSpace.xs,
            runSpacing: AppSpace.xs,
            children: [
              for (final h in hosts)
                Chip(
                  // Adresse intégrale + sélectionnable : les 56 chars
                  // portent la clé publique et le checksum — les
                  // tronquer empêcherait de vérifier la destination.
                  label: SelectableText(
                    h,
                    style: AppTypography.mono(fontSize: 11),
                  ),
                  visualDensity: VisualDensity.compact,
                ),
            ],
          ),
        ],
        if (backends.isNotEmpty) ...[
          const SizedBox(height: AppSpace.xs),
          Text(l10n.torBackends, style: theme.textTheme.bodySmall),
          const SizedBox(height: AppSpace.xs),
          Wrap(
            spacing: AppSpace.xs,
            runSpacing: AppSpace.xs,
            children: [
              for (final b in backends)
                Chip(
                  label: Text(b),
                  visualDensity: VisualDensity.compact,
                ),
            ],
          ),
        ],
      ],
    );
  }
}
