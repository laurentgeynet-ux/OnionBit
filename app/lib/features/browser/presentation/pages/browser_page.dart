// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Page « Navigateur » — point d'entrée visible du navigateur
/// `.onion` (ADR-0030) : état de la lane, champ d'adresse et
/// lancement. Les hôtes allowlistés (config) sont rappelés en
/// raccourcis — le daemon re-valide chaque cible à l'envoi.
library;

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:window_manager/window_manager.dart';

import '../../../../core/design/design_tokens.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../diagnostic/domain/diagnostic_models.dart';
import '../../../diagnostic/presentation/providers/diagnostic_providers.dart';
import '../../../settings/presentation/providers/settings_providers.dart';
import '../../../settings/presentation/widgets/settings_section.dart';
import '../../browser_launch.dart';
import '../widgets/browser_embed.dart';

/// DuckDuckGo Lite sur `.onion` — recherche HTML sans JS, reste sur
/// l'oignon (la racine meta-refresh vers clearnet : cul-de-sac sous
/// politique `off`). Raccourci fixe — toujours présent, comme le
/// lien de la page d'accueil `about:onionbit`.
const _kDdgLiteOnion =
    'https://duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion/lite/';

/// Noms d'affichage des `.onion` connus — une puce « The Pirate
/// Bay » parle mieux que 56 chars. L'adresse intégrale reste en
/// tooltip au survol : elle porte la clé publique et le checksum,
/// la masquer partout empêcherait de vérifier la destination.
const _kOnionDisplayNames = {
  'piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion':
      'The Pirate Bay',
};

class BrowserPage extends ConsumerStatefulWidget {
  const BrowserPage({super.key});

  @override
  ConsumerState<BrowserPage> createState() => _BrowserPageState();
}

class _BrowserPageState extends ConsumerState<BrowserPage>
    with WindowListener {
  final _addr = TextEditingController();
  final _regionKey = GlobalKey();
  // Statique : l'hôte survit au `dispose` de la page — changer
  // d'onglet masque la fenêtre adoptée (`hide`) au lieu de la
  // détacher en externe flottante, et le retour sur l'onglet la
  // révèle avec sa session intacte.
  static final _embedHost = BrowserEmbedHost();
  Timer? _pollTimer;

  /// Garde qui survit à la page : la fermeture de la fenêtre de
  /// l'app ferme aussi le navigateur embarqué — un navigateur
  /// confiné (profil éphémère) n'a pas de raison de rester orphelin.
  static final _closeGuard = _EmbedCloseGuard();
  static var _closeGuardRegistered = false;

  @override
  void initState() {
    super.initState();
    if (_embedHost.childHwnd != 0) {
      if (_embedHost.alive()) {
        // Retour sur l'onglet : la fenêtre masquée est re-positionnée
        // sur la zone puis réaffichée (post-frame : il faut un
        // `RenderBox` dimensionné pour `syncTo`).
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (!mounted) return;
          windowManager.addListener(this);
          _pollTimer ??= Timer.periodic(
            const Duration(milliseconds: 80),
            (_) => _poll(),
          );
          _syncEmbed();
          _embedHost.reveal();
        });
      } else {
        // Le navigateur est mort pendant l'absence — état propre
        // (sinon `_adopted` périmé bloque la ré-adoption suivante)
        // puis retour au tableau de bord.
        _embedHost.forget();
      }
    }
  }

  @override
  void dispose() {
    _pollTimer?.cancel();
    _pollTimer = null;
    windowManager.removeListener(this);
    // Quitter l'onglet masque la fenêtre adoptée — la session
    // navigateur survit et sera révélée au retour sur l'onglet.
    // Le bouton « détacher » (`release`) reste la sortie externe.
    _embedHost.hide();
    _addr.dispose();
    super.dispose();
  }

  /// Champ vide = page d'accueil locale `about:onionbit` (aucune
  /// connexion) ; hôte nu → `http://` pour un `.onion`, `https://`
  /// pour un hôte clearnet (schéma choisi pour la politique de
  /// sortie),
  /// sinon l'URL saisie — la route re-valide schéma et cible.
  /// Windows : lancement embarqué (`--host-shell` + reparenting) ;
  /// autres plateformes : fenêtre externe.
  void _open() {
    final raw = _addr.text.trim();
    // Hôte nu : `.onion` → HTTP (Tor chiffre déjà le transport —
    // les services onion sont majoritairement en HTTP) ; clearnet →
    // HTTPS — `allow_clearnet_exit = https-only` n'accepte que
    // CONNECT :443, un `http://` serait refusé par la politique.
    final url = raw.isEmpty
        ? ''
        : raw.startsWith('http')
        ? raw
        : raw.endsWith('.onion')
        ? 'http://$raw/'
        : 'https://$raw/';
    _openUrl(url);
  }

  /// Lance `url` par le même chemin que le champ d'adresse —
  /// embarquée sous Windows (`--host-shell`), externe ailleurs.
  /// Les raccourcis (`_QuickLinks`) passent par là : appeler
  /// `launchOnionBrowser` directement ouvrirait une fenêtre
  /// externe alors que l'onglet sait héberger le navigateur.
  void _openUrl(String url) {
    if (Platform.isWindows) {
      _openEmbedded(url);
    } else {
      launchOnionBrowser(context, ref, url);
    }
  }

  /// Lance `--host-shell` puis adopte le HWND publié — la zone est
  /// posée par le `setState` et la fenêtre adoptée en post-frame
  /// (il faut un `RenderBox` dimensionné pour `syncTo`). Repli
  /// externe silencieux si `hwnd` est `null` (déjà snackbaré).
  Future<void> _openEmbedded(String url) async {
    final hwnd = await launchOnionBrowserEmbedded(context, ref, url);
    if (hwnd == null || !mounted) return;
    _embedHost.childHwnd = hwnd;
    setState(() {});
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _embedHost.childHwnd == 0) return;
      if (_embedHost.adopt(hwnd)) {
        // Garde de fermeture : une fois par adoption — elle survit
        // à la page pour fermer le navigateur à la sortie de l'app.
        if (!_closeGuardRegistered) {
          windowManager.addListener(_closeGuard);
          _closeGuardRegistered = true;
        }
        _syncEmbed();
        // Retries post-adoption : la géométrie initiale de la
        // fenêtre peut différer du rect final de la zone — couvre
        // la course entre l'adoption et le premier layout Flutter.
        for (final d in const [150, 600, 1200]) {
          Future<void>.delayed(
            Duration(milliseconds: d),
            _syncEmbed,
          );
        }
        // Suivi temps réel du drag : `windowManager` relaie les
        // `WM_MOVE`/`WM_SIZE` du runner → `onWindowMove`/`Resize` →
        // `_syncEmbed`. Sans ça la fenêtre trackée ne se met à jour
        // qu'au rythme du poll (~80 ms) → traîne visible au drag.
        windowManager.addListener(this);
        // ~80 ms : filet de sécurité pour les déplacements hors
        // événements (snap clavier, snap assist) + détection de
        // mort du navigateur. Le sync est dédupliqué : pas de
        // `SetWindowPos` si rien n'a bougé.
        _pollTimer ??= Timer.periodic(
          const Duration(milliseconds: 80),
          (_) => _poll(),
        );
      } else {
        // Reparent refusé → fenêtre externe, retour tableau de bord.
        _embedHost.childHwnd = 0;
        setState(() {});
      }
    });
  }

  /// Relais `windowManager` → `WM_MOVE` pendant le drag de la
  /// fenêtre hôte : re-sync immédiat de la fenêtre trackée.
  @override
  void onWindowMove() => _syncEmbed();

  /// `WM_SIZE` pendant un resize de l'hôte — idem, temps réel.
  @override
  void onWindowResize() => _syncEmbed();

  /// Mort du navigateur (croix propre à l'app, Ctrl+Q, crash) →
  /// retour au tableau de bord ; sinon re-sync du rect (pas cher,
  /// no-op si inchangé).
  void _poll() {
    if (!_embedHost.alive()) {
      _pollTimer?.cancel();
      _pollTimer = null;
      windowManager.removeListener(this);
      // `forget` (pas juste `childHwnd = 0`) : `_adopted`, `_hostHwnd`
      // et le cache de rect doivent être oubliés pour que la
      // prochaine `adopt` refasse la chirurgie complète.
      _embedHost.forget();
      if (mounted) setState(() {});
      return;
    }
    _syncEmbed();
  }

  /// Suit le rect courant de la zone embed (coords client =
  /// pixels physiques du FlutterView, d'où le × dpr).
  void _syncEmbed() {
    if (!_embedHost.embedded) return;
    final box = _regionKey.currentContext?.findRenderObject();
    if (box is! RenderBox || !box.hasSize || box.size.isEmpty) return;
    final dpr = MediaQuery.of(context).devicePixelRatio;
    final pos = box.localToGlobal(Offset.zero) * dpr;
    final size = box.size * dpr;
    _embedHost.syncTo(
      pos.dx.round(),
      pos.dy.round(),
      size.width.round(),
      size.height.round(),
    );
  }

  /// Détache la fenêtre — elle devient externe, l'onglet retourne
  /// au tableau de bord (relancer ré-embarquera une nouvelle
  /// instance).
  void _detach() {
    _pollTimer?.cancel();
    _pollTimer = null;
    windowManager.removeListener(this);
    _embedHost.release();
    setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final status = ref.watch(torStatusProvider).value;
    final settings = ref.watch(daemonSettingsProvider).value ?? const {};
    if (_embedHost.childHwnd != 0) {
      // Navigateur embarqué : barre fine (détacher / fermer — la
      // fenêtre n'a plus de croix native en mode non décoré) + la
      // zone qui reçoit le HWND reparenté.
      return Column(
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: AppSpace.md,
              vertical: AppSpace.xs,
            ),
            child: Row(
              children: [
                const Icon(Icons.language_outlined, size: 18),
                const SizedBox(width: AppSpace.sm),
                Text(l10n.navBrowser),
                const Spacer(),
                IconButton(
                  tooltip: l10n.browserDetach,
                  icon: const Icon(Icons.open_in_new, size: 18),
                  onPressed: _detach,
                ),
                IconButton(
                  tooltip: l10n.close,
                  icon: const Icon(Icons.close, size: 18),
                  onPressed: _embedHost.close,
                ),
              ],
            ),
          ),
          Expanded(
            child: NotificationListener<SizeChangedLayoutNotification>(
              onNotification: (_) {
                _syncEmbed();
                return false;
              },
              child: SizeChangedLayoutNotifier(
                child: Container(
                  key: _regionKey,
                  // `bg_color` Dillo par défaut (0xdcd1ba) : le
                  // « airspace gap » — une frame où le HWND enfant
                  // n'est pas encore recomposé au-dessus de la
                  // frame Flutter — montre ce fond au lieu d'un
                  // rectangle noir bien visible.
                  color: const Color(0xFFDCD1BA),
                ),
              ),
            ),
          ),
        ],
      );
    }
    return ListView(
      padding: const EdgeInsets.all(AppSpace.md),
      children: [
        // État de la lane — « prêt » est un état honnête, jamais
        // déduit : il vient de `/api/tor/status`.
        _StatusCard(status: status),
        const SizedBox(height: AppSpace.md),
        // Champ d'adresse : hôte nu → `http://<hôte>/`, sinon l'URL
        // saisie — la route re-valide schéma et cible.
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _addr,
                onSubmitted: (_) => _open(),
                decoration: InputDecoration(
                  labelText: l10n.navBrowser,
                  hintText: l10n.browserAddressHint,
                  prefixIcon: const Icon(Icons.language_outlined),
                  // Bordure explicite + fond : sans elle le champ rend
                  // comme un libellé — rien n'indique qu'on peut y
                  // taper une adresse.
                  border: const OutlineInputBorder(),
                  filled: true,
                  // Label toujours flottant : sinon le hint
                  // n'apparaît qu'au focus — champ vide muet.
                  floatingLabelBehavior: FloatingLabelBehavior.always,
                ),
              ),
            ),
            const SizedBox(width: AppSpace.sm),
            FilledButton.icon(
              onPressed: _open,
              icon: const Icon(Icons.open_in_new, size: 18),
              label: Text(l10n.browserOpen),
            ),
          ],
        ),
        const SizedBox(height: AppSpace.lg),
        // Raccourcis : DDG Lite fixe + hôtes allowlistés (config).
        _QuickLinks(settings: settings, onLaunch: _openUrl),
        const SizedBox(height: AppSpace.lg),
        // Réglages du navigateur directement sur l'onglet — le
        // parcours utilisateur ne doit pas exiger Réglages→Réseau.
        _BrowserSettingsCard(settings: settings),
      ],
    );
  }
}

/// `WindowListener` qui survit à `_BrowserPageState` : quand la
/// fenêtre de l'app se ferme, `WM_CLOSE` est posté au navigateur
/// embarqué — sinon il resterait orphelin à l'écran alors que son
/// profil est éphémère et son processus confiné.
class _EmbedCloseGuard with WindowListener {
  @override
  void onWindowClose() => _BrowserPageState._embedHost.close();
}

/// État Tor + proxy navigateur — lecture seule de
/// `/api/tor/status`.
class _StatusCard extends StatelessWidget {
  const _StatusCard({required this.status});

  final TorRuntimeStatus? status;

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    final s = status ?? TorRuntimeStatus.disabled;
    final proxy = s.browserProxy;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(AppSpace.md),
        child: Wrap(
          spacing: AppSpace.sm,
          runSpacing: AppSpace.sm,
          children: [
            Chip(
              avatar: Icon(
                s.state == TorRuntimeState.ready
                    ? Icons.check_circle_outline
                    : Icons.schedule,
                size: 16,
              ),
              label: Text('Tor : ${s.state.name}${s.progress < 100 && s.state == TorRuntimeState.bootstrapping ? ' ${s.progress}%' : ''}'),
            ),
            if (proxy.enabled)
              Chip(
                avatar: const Icon(Icons.vpn_lock, size: 16),
                label: Text(
                  proxy.listen != null
                      ? l10n.torBrowserListen(proxy.listen!)
                      : l10n.torBrowserConns(proxy.connsActive),
                ),
              )
            else
              Chip(
                avatar: const Icon(Icons.block, size: 16),
                label: Text(l10n.torBrowserProxy),
              ),
          ],
        ),
      ),
    );
  }
}

/// Raccourcis de lancement : DuckDuckGo Lite `.onion` (fixe — le
/// moteur de découverte no-JS qui reste sur l'oignon) puis les hôtes
/// allowlistés (`tor.allowed_hosts` ∪ miroirs des backends, même
/// union que `_TorAllowlist`). Chaque puce délègue à `onLaunch` —
/// le chemin de la page (embarqué sous Windows, externe ailleurs) ;
/// `launchOnionBrowser` ici ouvrirait une fenêtre externe alors
/// que l'onglet sait héberger le navigateur.
class _QuickLinks extends StatelessWidget {
  const _QuickLinks({required this.settings, required this.onLaunch});

  final Map<String, dynamic> settings;

  /// `url` complète à lancer — fournie par `_BrowserPageState` pour
  /// rester sur le même chemin que le champ d'adresse.
  final void Function(String url) onLaunch;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    final tor = settings['tor'];
    final search = tor is Map ? tor['search'] : null;
    final backendsMap = search is Map && search['backends'] is Map
        ? search['backends'] as Map
        : const {};
    final hosts = <String>{
      if (tor is Map && tor['allowed_hosts'] is List)
        for (final h in tor['allowed_hosts'] as List) '$h',
      for (final b in backendsMap.values)
        if (b is Map && b['mirrors'] is List)
          for (final h in b['mirrors'] as List) '$h',
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          l10n.torAllowedHosts,
          style: theme.textTheme.labelMedium,
        ),
        const SizedBox(height: AppSpace.xs),
        Wrap(
          spacing: AppSpace.xs,
          runSpacing: AppSpace.xs,
          children: [
            ActionChip(
              avatar: const Icon(Icons.travel_explore, size: 14),
              label: const Text('DuckDuckGo Lite'),
              onPressed: () => onLaunch(_kDdgLiteOnion),
            ),
            for (final h in hosts)
              Tooltip(
                // Adresse intégrale au survol — vérifiable même
                // quand la puce montre un nom d'affichage.
                message: h,
                child: ActionChip(
                  avatar: const Icon(Icons.open_in_new, size: 14),
                  label: Text(
                    _kOnionDisplayNames[h] ?? h,
                    style: theme.textTheme.bodySmall?.copyWith(
                      fontFamily: _kOnionDisplayNames.containsKey(h)
                          ? null
                          : AppFontFamilies.mono,
                    ),
                  ),
                  onPressed: () => onLaunch('http://$h/'),
                ),
              ),
          ],
        ),
      ],
    );
  }
}

/// Réglages du navigateur confiné — portés sur l'onglet pour que le
/// parcours utilisateur reste local : activer le proxy et choisir la
/// politique de sortie clearnet sans aller dans Réglages→Réseau.
/// Les patches partent immédiatement (`applySettingsPatch`).
class _BrowserSettingsCard extends ConsumerStatefulWidget {
  const _BrowserSettingsCard({required this.settings});

  final Map<String, dynamic> settings;

  @override
  ConsumerState<_BrowserSettingsCard> createState() =>
      _BrowserSettingsCardState();
}

class _BrowserSettingsCardState
    extends ConsumerState<_BrowserSettingsCard> {
  String _clearnetExit = 'off';
  bool _synced = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    if (!_synced) {
      _clearnetExit = settingsString(widget.settings, const [
        'tor',
        'browser_proxy',
        'allow_clearnet_exit',
      ]);
      if (!const {'off', 'https-only', 'on'}.contains(_clearnetExit)) {
        _clearnetExit = 'off';
      }
      _synced = true;
    }
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(AppSpace.md),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SettingsSwitch(
              path: const ['tor', 'browser_proxy', 'enabled'],
              value: settingsBool(widget.settings, const [
                'tor',
                'browser_proxy',
                'enabled',
              ]),
              title: l10n.torBrowserProxy,
              subtitle: l10n.torBrowserProxyHint,
              defaultValue: true,
            ),
            const SizedBox(height: AppSpace.xs),
            SizedBox(
              width: 320,
              child: DropdownButtonFormField<String>(
                initialValue: _clearnetExit,
                isExpanded: true,
                decoration: InputDecoration(labelText: l10n.torExitLabel),
                items: [
                  DropdownMenuItem(value: 'off', child: Text(l10n.torExitOff)),
                  DropdownMenuItem(
                    value: 'https-only',
                    child: Text(l10n.torExitHttpsOnly),
                  ),
                  DropdownMenuItem(value: 'on', child: Text(l10n.torExitOn)),
                ],
                onChanged: (v) {
                  setState(() => _clearnetExit = v ?? 'off');
                  applySettingsPatch(context, ref, {
                    'tor': {
                      'browser_proxy': {'allow_clearnet_exit': _clearnetExit},
                    },
                  });
                },
              ),
            ),
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                switch (_clearnetExit) {
                  'https-only' => l10n.torExitHttpsOnlyHint,
                  'on' => l10n.torExitOnHint,
                  _ => l10n.torExitOffHint,
                },
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
