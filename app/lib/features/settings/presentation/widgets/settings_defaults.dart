/// Défauts documentés des réglages — mêmes valeurs que les `Default`
/// du backend (`onionbit-core/src/config.rs`,
/// `onionbit-bittorrent/src/config.rs`, `onionbit-tunnel/src/settings.rs`).
/// Consommés par le bouton « Défauts » des sections : patch merge
/// envoyé tel quel à `POST /api/settings`.
library;

/// Bande passante — `0` = illimité.
const kBandwidthDefaults = <String, dynamic>{
  'libtorrent': {'max_download_rate': 0, 'max_upload_rate': 0},
};

/// File d'attente — défauts Tribler (3/5/1/500, `-1` = illimité).
const kQueueDefaults = <String, dynamic>{
  'libtorrent': {
    'active_downloads': 3,
    'active_seeds': 5,
    'active_checking': 1,
    'active_limit': 500,
  },
};

/// Politique de seed/anonymat des nouveaux téléchargements —
/// `DownloadDefaults::default()`.
const kSeedingDefaults = <String, dynamic>{
  'libtorrent': {
    'download_defaults': {
      'seeding_mode': 'forever',
      'seeding_ratio': 2.0,
      'seeding_time': 60.0,
      'anonymity_enabled': true,
      'number_hops': 1,
      'safeseeding_enabled': true,
    },
  },
};

/// Tunnels anonymes — `TunnelSettings::default()` + activation.
const kAnonymityDefaults = <String, dynamic>{
  'tunnel_community': {
    'enabled': true,
    'min_circuits': 1,
    'max_circuits': 8,
    'exitnode_enabled': false,
  },
};

/// Découverte/proxy — `EngineConfig::default()` (tout activé, pas de
/// proxy).
const kNetworkDefaults = <String, dynamic>{
  'libtorrent': {
    'dht': true,
    'upnp': true,
    'natpmp': true,
    'lsd': true,
    'utp': true,
    'proxy_type': 0,
  },
};

/// Automatisation — pas de watch folder ni de flux RSS au départ.
const kAutomationDefaults = <String, dynamic>{
  'watch_folder': {'enabled': false, 'check_interval': 10},
  'rss': {'enabled': false, 'urls': <String>[]},
};
