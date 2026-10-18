// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Modèles du panneau Diagnostic — miroirs de `CircuitInfo`,
/// `RelayInfo`, `ExitInfo`, `SwarmInfo`, `TunnelPeerInfo`
/// (`onionbit-tunnel`) et de `overlays` (`onionbit-ipv8`).
class OverlayInfo {
  const OverlayInfo({
    required this.name,
    required this.id,
    required this.peers,
  });

  final String name;
  final String id;
  final int peers;
}

class CircuitInfo {
  const CircuitInfo({
    required this.id,
    required this.goalHops,
    required this.actualHops,
    required this.type,
    required this.state,
    required this.bytesUp,
    required this.bytesDown,
    required this.exitFlags,
    required this.infoHash,
    required this.verifiedHops,
    required this.unverifiedHop,
    required this.creationTime,
  });

  final int id;
  final int goalHops;
  final int actualHops;
  final String type;
  final String state;
  final int bytesUp;
  final int bytesDown;
  final int exitFlags;
  final String? infoHash;

  /// `verified_hops` : mid hex de chaque saut vérifié, dans l'ordre
  /// du circuit (la route réellement prise).
  final List<String> verifiedHops;

  /// `unverified_hop` : mid hex du saut en cours d'ajout (`''` sinon).
  final String unverifiedHop;

  /// `creation_time` (epoch secondes).
  final int creationTime;

  bool get ready => state == 'READY';
}

class RelayInfo {
  const RelayInfo({
    required this.circuitIn,
    required this.circuitOut,
    required this.rendezvous,
    required this.bytesUp,
    required this.bytesDown,
  });

  final int circuitIn;
  final int circuitOut;
  final bool rendezvous;
  final int bytesUp;
  final int bytesDown;
}

class ExitInfo {
  const ExitInfo({required this.circuitId, required this.enabled});

  final int circuitId;
  final bool enabled;
}

class SwarmInfo {
  const SwarmInfo({
    required this.infoHash,
    required this.connections,
    required this.seeder,
  });

  final String infoHash;
  final int connections;
  final bool seeder;
}

class TunnelPeerInfo {
  const TunnelPeerInfo({
    required this.ip,
    required this.port,
    required this.mid,
    required this.isKeyCompatible,
    required this.flags,
  });

  final String ip;
  final int port;
  final String mid;
  final bool isKeyCompatible;

  /// `PEER_FLAG_*` annoncés — liste d'entiers (set Python), pas un
  /// bitmask.
  final List<int> flags;
}

/// Point d'introduction d'un swarm caché (`IntroductionPoint.to_dict`
/// pyipv8) — entrées des vues « Pairs DHT » et « Pairs PEX ».
class IntroPoint {
  const IntroPoint({
    required this.ip,
    required this.port,
    required this.publicKey,
    required this.seederPk,
    required this.source,
  });

  final String ip;
  final int port;
  final String publicKey;
  final String seederPk;

  /// Origine de la découverte (`dht`, `peer_discovery`/`pex`…).
  final String source;
}

/// Points d'introduction regroupés par info-hash de swarm
/// (`GET /api/ipv8/tunnel/peers/{dht,pex}` → `[{info_hash, peers}]`).
class SwarmPeers {
  const SwarmPeers({required this.infoHash, required this.peers});

  final String infoHash;
  final List<IntroPoint> peers;
}

/// Statistiques générales du daemon (`GET /api/statistics/tribler` →
/// `tribler_statistics`).
class OnionbitStats {
  const OnionbitStats({
    required this.dbSize,
    required this.numTorrents,
    required this.peers,
    required this.sessions,
    required this.version,
    required this.uptimeSec,
    required this.totalRecvBytes,
    required this.totalSentBytes,
    required this.laneHops,
  });

  final int dbSize;
  final int numTorrents;

  /// Pairs découverts par la stack IPv8 (`-1` = stack inactive).
  final int peers;

  /// Nombre de sessions moteur (principale + lanes anonymes ; `-1`
  /// = stack inactive).
  final int sessions;
  final String version;

  /// Secondes depuis le démarrage du daemon (`-1` = champ absent,
  /// daemon plus ancien).
  final int uptimeSec;

  /// Totaux session des moteurs BitTorrent (`libtorrent.total_*_bytes`,
  /// `-1` = stack inactive) — remis à zéro au redémarrage.
  final int totalRecvBytes;
  final int totalSentBytes;

  /// Sauts des lanes anonymes actives (`socks5_sessions[].hops`,
  /// vide = aucune / tunnel inactif).
  final List<int> laneHops;
}

/// Compteurs d'octets de l'endpoint IPv8 (`GET /api/statistics/ipv8`
/// → `ipv8_statistics`) + débits calculés par le daemon
/// (`rate_up`/`rate_down`, extension Rust).
class Ipv8Traffic {
  const Ipv8Traffic({
    required this.up,
    required this.down,
    this.rateUp = 0,
    this.rateDown = 0,
    this.bandwidth,
  });

  /// Octets émis par l'endpoint overlay depuis le démarrage.
  final int up;

  /// Octets reçus par l'endpoint overlay depuis le démarrage.
  final int down;

  /// Débit montant instantané mesuré par le daemon (o/s,
  /// fenêtre glissante de l'endpoint — `rate_up`).
  final int rateUp;

  /// Débit descendant instantané mesuré par le daemon (o/s —
  /// `rate_down`).
  final int rateDown;

  /// Mesure de capacité upload + plafond servi (`bandwidth` —
  /// extension Rust, `tunnel_community/bandwidth`).
  final RelayBandwidth? bandwidth;
}

/// Plafond du trafic servi aux autres pairs + signal RTT du
/// contrôleur de congestion (`ipv8_statistics.bandwidth` —
/// `services/bandwidth.rs`, extension Rust).
class RelayBandwidth {
  const RelayBandwidth({
    required this.effectiveRelayBps,
    required this.baseRttMs,
    required this.minRttMs,
    required this.rttSamples,
    required this.relayMode,
    required this.relayDropped,
    required this.servedBps,
    required this.servedBytes,
  });

  /// Plafond servi actuellement appliqué au tunnel (octets/s).
  final int effectiveRelayBps;

  /// Baseline RTT (ms) — min glissant des minimums de rafale ;
  /// `null` tant qu'aucun pong n'a été reçu.
  final double? baseRttMs;

  /// RTT minimum de la dernière rafale (ms) ; `null` idem.
  final double? minRttMs;

  /// Pongs exploités au dernier tick.
  final int rttSamples;

  /// Mode de `tunnel_community/max_relayed_rate` : `auto` (-1),
  /// `unlimited` (0), `fixed` (>0) — détermine l'affichage (« — » vs
  /// « illimité ») quand `effectiveRelayBps` vaut 0.
  final String relayMode;

  /// Datagrammes servis perdus faute de budget.
  final int relayDropped;

  /// Débit servi mesuré au limiteur (octets/s) — la valeur exacte du
  /// trafic relayé/sorti pour les autres, sans soustraction.
  final int servedBps;

  /// Octets servis cumulés depuis le démarrage.
  final int servedBytes;
}

/// Une connexion BitTorrent vers un pair (entrée `bittorrent[]` de
/// `GET /api/connections`, extension Rust).
class BtPeerConn {
  const BtPeerConn({
    required this.infohash,
    required this.connKind,
    required this.state,
    required this.incoming,
    required this.client,
    required this.bytesUp,
    required this.bytesDown,
  });

  final String infohash;

  /// Transport rqbit (`tcp`, `uTP`, `socks` ; `''` si inconnu).
  final String connKind;

  /// État interne rqbit (`live`, `connecting`, …).
  final String state;

  /// Connexion initiée par le pair distant.
  final bool incoming;

  /// Nom du client distant (peer-id décodé, `''` si inconnu).
  final String client;
  final int bytesUp;
  final int bytesDown;
}

/// Adresse distante agrégée (`GET /api/connections`) : un `ip:port`
/// avec tous les rôles/protocoles observés par le daemon.
class ConnectionInfo {
  const ConnectionInfo({
    required this.ip,
    required this.port,
    required this.transports,
    required this.ipv8,
    required this.mid,
    required this.overlays,
    required this.dht,
    required this.tunnelFlags,
    required this.exitCircuits,
    required this.bittorrent,
  });

  final String ip;
  final int port;

  /// Transports dérivés (`udp`, `tcp`).
  final List<String> transports;

  /// Pair vérifié du `Network` IPv8.
  final bool ipv8;

  /// `mid` hex du pair IPv8 (`''` sinon).
  final String mid;

  /// Noms d'overlays dont le pair est membre.
  final List<String> overlays;

  /// Présent dans la table de routage DHT.
  final bool dht;

  /// `PEER_FLAG_*` annoncés (pair tunnel).
  final List<int> tunnelFlags;

  /// `circuit_id` des sockets de sortie ayant contacté cette cible WAN.
  final List<int> exitCircuits;

  /// Connexions BitTorrent vers ce pair.
  final List<BtPeerConn> bittorrent;
}

/// Socket d'écoute locale (`listeners[]` de `GET /api/connections`).
class ListenerInfo {
  const ListenerInfo({
    required this.protocol,
    required this.address,
    this.circuitId,
    this.hops,
  });

  /// `ipv8-udp`, `ipv8-udp-v6`, `tunnel-exit-udp`, `socks5`,
  /// `bittorrent`.
  final String protocol;
  final String address;

  /// Positionné pour `tunnel-exit-udp`.
  final int? circuitId;

  /// Positionné pour `socks5` (nombre de sauts de la lane).
  final int? hops;
}

/// Corps complet de `GET /api/connections`.
class ConnectionsReport {
  const ConnectionsReport({required this.connections, required this.listeners});

  final List<ConnectionInfo> connections;
  final List<ListenerInfo> listeners;
}

/// Pair de la communauté d'extension OnionBit (`GET /api/ipv8/ext` →
/// `peers[]`) — reconnu par `hello` signé, pas de légacy Tribler.
class ExtPeerInfo {
  const ExtPeerInfo({
    required this.mid,
    required this.pk,
    required this.capsNames,
    required this.lastHelloSecs,
  });

  /// `mid` hex du pair.
  final String mid;

  /// Clé publique complète (`pk_bin` hex) — adressable en
  /// messagerie / attestation `identity`.
  final String pk;

  /// Capacités annoncées décodées (`caps_names` : `obf_v1`,
  /// `msg_v1`…). Vide = pair ext sans capacité annoncée.
  final List<String> capsNames;

  /// Secondes depuis le dernier `hello` valide reçu.
  final int lastHelloSecs;
}

/// Instantané de la communauté ext (`GET /api/ipv8/ext`, ADR-0015).
class ExtInfo {
  const ExtInfo({
    required this.enabled,
    required this.capsNames,
    required this.peers,
  });

  /// `ext/enabled` effectif.
  final bool enabled;

  /// Capacités annoncées localement (`caps_names`).
  final List<String> capsNames;

  /// Pairs OnionBit reconnus.
  final List<ExtPeerInfo> peers;
}

/// Lien bilatéral du registre ext (`GET /api/ipv8/ext/ledger` →
/// `links[]`) — positions `seq_a`/`seq_b` signées des deux côtés.
class ExtLedgerLink {
  const ExtLedgerLink({
    required this.pkAMid,
    required this.seqA,
    required this.pkBMid,
    required this.seqB,
    required this.sealed,
    required this.hash,
  });

  final String pkAMid;
  final int seqA;
  final String pkBMid;
  final int seqB;

  /// `true` = lien complet (deux signatures — reglable), `false` =
  /// proposition en vol.
  final bool sealed;
  final String hash;
}

/// Registre bilatéral ext (`GET /api/ipv8/ext/ledger`, ADR-0015 §5).
class ExtLedger {
  const ExtLedger({
    required this.enabled,
    required this.linksCount,
    required this.pending,
    required this.forks,
    required this.myHeadSeq,
    required this.myHeadHash,
    required this.links,
  });

  final bool enabled;
  final int linksCount;
  final int pending;
  final int forks;

  /// Position et hash de notre tête de chaîne.
  final int myHeadSeq;
  final String myHeadHash;
  final List<ExtLedgerLink> links;
}

/// Attestation de curation stockée (`GET /api/ipv8/ext/attestations`,
/// ADR-0015 §6) — signée par un curateur, verdict ±1.
class ExtAttestation {
  const ExtAttestation({
    required this.curator,
    required this.curatorMid,
    required this.kind,
    required this.subject,
    required this.verdict,
    required this.ts,
  });

  /// Clé publique complète du curateur (hex) — nécessaire pour le
  /// suivre (`ext/curators` attend la clé entière, pas le `mid`).
  final String curator;

  /// `mid` hex du curateur signataire.
  final String curatorMid;

  /// `infohash` | `channel`.
  final String kind;

  /// Sujet hex (info-hash ou clé LibNaCl du canal).
  final String subject;

  /// `endorse` | `flag`.
  final String verdict;

  /// Horodatage unix de l'attestation.
  final int ts;
}

/// Score de confiance local d'un sujet
/// (`GET /api/ipv8/ext/trust/{kind}/{subject}`, ADR-0015 §6) —
/// alimenté uniquement par les curateurs suivis.
class ExtTrust {
  const ExtTrust({
    required this.score,
    required this.endorsements,
    required this.flags,
    required this.attestationCount,
  });

  /// Somme des verdicts des curateurs suivis (+1/-1).
  final int score;
  final List<String> endorsements;
  final List<String> flags;

  /// Attestations stockées sur le sujet (toutes curatrices —
  /// visibilité, pas comptées dans `score`).
  final int attestationCount;
}

/// Échantillon de débit d'un speed test de circuit (MiB/s,
/// `speed: {"up", "down"}` pyipv8).
class SpeedSample {
  const SpeedSample({required this.up, required this.down});

  final double up;
  final double down;
}

/// État de la lane anonyme pour la barre d'état — « honnête » :
/// prête = au moins un circuit `READY`, attente = circuits absents,
/// désactivée = stack IPv8/tunnel inactive (erreur API).
enum AnonLaneState { disabled, waiting, ready }

class AnonLaneStatus {
  const AnonLaneStatus({
    required this.state,
    required this.readyCircuits,
    required this.totalCircuits,
    this.minReadyHops = 0,
  });

  final AnonLaneState state;
  final int readyCircuits;
  final int totalCircuits;

  /// Profondeur réelle (`actual_hops`, route vérifiée) du circuit
  /// READY le *moins* profond — posture « pire cas » affichée par le
  /// HUD d'anonymat (ADR-0021 §8) ; 0 quand aucun circuit n'est prêt.
  final int minReadyHops;

  static const disabled = AnonLaneStatus(
    state: AnonLaneState.disabled,
    readyCircuits: 0,
    totalCircuits: 0,
  );
}

/// État du runtime Tor (`GET /api/tor/status`, ADR-0028 §5) —
/// « honnête » comme `AnonLaneStatus` : `off`/`disabled` ≠ panne.
enum TorRuntimeState { off, bootstrapping, ready, dormant, error, unknown }

class TorRuntimeStatus {
  const TorRuntimeStatus({
    required this.enabled,
    required this.mode,
    required this.state,
    required this.progress,
    this.externalSocks5,
    this.error,
    this.sources = const [],
    this.browserProxy = BrowserProxyStatus.off,
  });

  /// `tor.enabled` — la fonctionnalité est opt-in, désactivée par
  /// défaut (bootstrap visible par le FAI, ADR-0028 §1).
  final bool enabled;

  /// `"embedded"` (arti embarqué) ou `"external"` (SOCKS5 système).
  final String mode;

  /// État observable du runtime.
  final TorRuntimeState state;

  /// Progression du bootstrap 0-100 (`bootstrapping` seulement).
  final int progress;

  /// Endpoint SOCKS5 configuré en mode `external` (`null` sinon).
  final String? externalSocks5;

  /// Message d'erreur du dernier bootstrap (`state == error`).
  final String? error;

  /// Identifiants des backends de recherche enregistrés
  /// (`tor.search.backends.<source>`).
  final List<String> sources;

  /// État du proxy navigateur loopback (`browser_proxy`, ADR-0030 §7).
  final BrowserProxyStatus browserProxy;

  /// Runtime absent ou non compilé (`tor` feature off) — l'endpoint
  /// répond toujours : état « off » honnête.
  static const disabled = TorRuntimeStatus(
    enabled: false,
    mode: 'embedded',
    state: TorRuntimeState.off,
    progress: 0,
  );

  static TorRuntimeStatus fromJson(Map<String, dynamic> j) => TorRuntimeStatus(
    enabled: j['enabled'] == true,
    mode: '${j['mode'] ?? 'embedded'}',
    state: switch ('${j['state'] ?? ''}') {
      'off' => TorRuntimeState.off,
      'bootstrapping' => TorRuntimeState.bootstrapping,
      'ready' => TorRuntimeState.ready,
      'dormant' => TorRuntimeState.dormant,
      'error' => TorRuntimeState.error,
      _ => TorRuntimeState.unknown,
    },
    progress: (j['progress'] as num?)?.toInt() ?? 0,
    externalSocks5: j['external_socks5'] is String
        ? j['external_socks5'] as String
        : null,
    error: j['error'] is String ? j['error'] as String : null,
    sources: [for (final s in (j['sources'] as List?) ?? const []) '$s'],
    browserProxy: j['browser_proxy'] is Map<String, dynamic>
        ? BrowserProxyStatus.fromJson(
            j['browser_proxy'] as Map<String, dynamic>,
          )
        : BrowserProxyStatus.off,
  );
}

/// État du proxy navigateur loopback (`tor/status → browser_proxy`,
/// ADR-0030 §7) — listener `127.0.0.1` pour le navigateur `.onion`
/// (dégradation `.onion` seulement ; clearnet via exit Tor tri-state).
class BrowserProxyStatus {
  const BrowserProxyStatus({
    required this.enabled,
    required this.listen,
    required this.connsActive,
  });

  /// Listener ouvert (`tor.browser_proxy.enabled` appliqué).
  final bool enabled;

  /// Adresse réellement liée (`127.0.0.1:port` — port éphémère
  /// résolu), `null` si non ouvert.
  final String? listen;

  /// Connexions navigateur actives (compteur RAII du proxy).
  final int connsActive;

  static const off = BrowserProxyStatus(
    enabled: false,
    listen: null,
    connsActive: 0,
  );

  static BrowserProxyStatus fromJson(Map<String, dynamic> j) =>
      BrowserProxyStatus(
        enabled: j['enabled'] == true,
        listen: j['listen'] is String ? j['listen'] as String : null,
        connsActive: (j['conns_active'] as num?)?.toInt() ?? 0,
      );
}

/// Résultat de la sonde de joignabilité d'un miroir `.onion`
/// (`GET /api/tor/probe`, ADR-0028) — un `ok:true` signifie qu'une
/// réponse HTTP a été reçue (le circuit de rendez-vous fonctionne,
/// quel que soit le statut).
class TorMirrorProbe {
  const TorMirrorProbe({
    required this.host,
    required this.ok,
    this.status,
    this.latencyMs,
    this.error,
  });

  final String host;
  final bool ok;
  final int? status;
  final int? latencyMs;
  final String? error;

  static TorMirrorProbe fromJson(Map<String, dynamic> j) => TorMirrorProbe(
    host: '${j['host'] ?? ''}',
    ok: j['ok'] == true,
    status: (j['status'] as num?)?.toInt(),
    latencyMs: (j['latency_ms'] as num?)?.toInt(),
    error: j['error'] is String ? j['error'] as String : null,
  );
}

/// Santé d'une source `.onion` — nombre de miroirs joignables +
/// détail par miroir (`reachable == 0` = source tombée/dépréciée,
/// visible sans avoir lancé une recherche).
class TorSourceProbe {
  const TorSourceProbe({
    required this.source,
    required this.reachable,
    required this.mirrors,
  });

  final String source;
  final int reachable;
  final List<TorMirrorProbe> mirrors;

  static TorSourceProbe fromJson(Map<String, dynamic> j) => TorSourceProbe(
    source: '${j['source'] ?? ''}',
    reachable: (j['reachable'] as num?)?.toInt() ?? 0,
    mirrors: [
      for (final m in (j['mirrors'] as List?) ?? const [])
        TorMirrorProbe.fromJson(m as Map<String, dynamic>),
    ],
  );
}
