//! Requetes HTTP(S) transportees par les cellules tunnel
//! (`HTTPRequestPayload` msg 28 / `HTTPResponsePayload` msg 29 —
//! `tribler/core/tunnel/payload.py`, `ipv8-rust-tunnels`).
//!
//! Usage Tribler : annonces de tracker et metadonnees a travers un
//! circuit. Le SOCKS5 `CONNECT` (`socks5.rs`) doit fournir un tunnel
//! TCP **transparent** au client (libtorrent / le client HTTP du
//! demandeur negocie lui-meme sa propre session TLS de bout en bout
//! quand la cible est `https://` — le relais ne doit jamais tenter de
//! parser le flux comme du HTTP en clair, ce qui echouerait
//! systematiquement des le `ClientHello`).
//!
//! Le format de cellule ne porte qu'un `identifier` (pas de
//! sequencement) : `http-request`/`http-response` sont donc reutilisees
//! en **flux continu** — chaque lecture cote demandeur (respectivement
//! cote sortie) part immediatement en cellule, sans attendre une
//! requete/reponse complete. `HttpResponse.total` est detourne en
//! marqueur de fin (`0` = le flux continue, `1` = dernier chunk, le
//! flux reel est ferme) plutot qu'un decompte de chunks connu a
//! l'avance — la sortie ne peut pas savoir a l'avance la taille d'une
//! reponse HTTPS (chiffree, taille imprevisible).

/// Taille max d'un chunk `http-request`/`http-response` (`socket.rs` :
/// 1400 octets — sous le MTU UDP pour traverser une cellule).
pub const HTTP_RESPONSE_CHUNK: usize = 1400;

/// Flux HTTP simultanes autorises par circuit de sortie (`exit.rs` :
/// semaphore de 5 permis) — un flux dure toute la connexion CONNECT,
/// pas juste un aller-retour.
pub const MAX_HTTP_REQUESTS_PER_CIRCUIT: usize = 5;

/// Timeout de la connexion TCP initiale vers la destination (exit).
pub const HTTP_CONNECT_TIMEOUT_MS: u64 = 10_000;

/// Inactivite maximale d'un flux CONNECT (ni lecture ni ecriture) cote
/// sortie avant fermeture forcee — un round-trip TLS complet a travers
/// 3 sauts sur le reseau public IPv8 peut prendre plusieurs secondes,
/// largement au-dela d'un aller-retour HTTP unique.
pub const HTTP_STREAM_IDLE_TIMEOUT_MS: u64 = 30_000;

/// Capacite du canal de chunks en attente d'ecriture (cote sortie,
/// requetes qui arrivent plus vite qu'elles ne sont ecrites) et des
/// chunks de reponse (cote demandeur).
pub const HTTP_STREAM_QUEUE: usize = 256;

/// Buffer de lecture du client SOCKS5 CONNECT avant le premier chunk
/// (`socks5.rs`).
pub const CONNECT_READ_CHUNK: usize = 16 * 1024;

/// Ouvre la connexion TCP reelle vers `target` (cote sortie) — aucune
/// tentative de TLS ni de parsing HTTP : le contenu relaye est opaque,
/// c'est au demandeur (client HTTP en amont) de negocier sa propre
/// session TLS de bout en bout a travers le tunnel si necessaire.
pub async fn connect_target(
    target: &tribler_ipv8::address::UdpAddress,
) -> Result<tokio::net::TcpStream, tribler_ipv8::error::Ipv8Error> {
    use tribler_ipv8::address::UdpAddress;
    Ok(match target {
        UdpAddress::Ipv4(a) => tokio::net::TcpStream::connect(std::net::SocketAddr::V4(*a)).await?,
        UdpAddress::Ipv6(a) => tokio::net::TcpStream::connect(std::net::SocketAddr::V6(*a)).await?,
        UdpAddress::Domain(host, port) => {
            tokio::net::TcpStream::connect((host.as_str(), *port)).await?
        }
    })
}
