//! Requetes HTTP transportees par les cellules tunnel
//! (`HTTPRequestPayload` msg 28 / `HTTPResponsePayload` msg 29 —
//! `tribler/core/tunnel/payload.py`, `ipv8-rust-tunnels`).
//!
//! Usage Tribler : annonces de tracker et metadonnees a travers un
//! circuit — le SOCKS5 CONNECT lit une requete HTTP brute du client,
//! l'envoie en cellule `http-request` a une sortie portant
//! `PEER_FLAG_EXIT_HTTP`, puis recolle les chunks `http-response`.

use std::io::ErrorKind;
use std::net::SocketAddr;

use onionbit_ipv8::address::UdpAddress;
use onionbit_ipv8::error::Ipv8Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Taille max d'un chunk `http-response` (`socket.rs` : 1400 octets —
/// sous le MTU UDP pour traverser une cellule).
pub const HTTP_RESPONSE_CHUNK: usize = 1400;

/// Requetes HTTP simultanees autorisees par circuit de sortie
/// (`exit.rs` : semaphore de 5 permis).
pub const MAX_HTTP_REQUESTS_PER_CIRCUIT: usize = 5;

/// Timeout d'une requete TCP de sortie (5 s comme `socket.rs`).
pub const HTTP_TCP_TIMEOUT_MS: u64 = 5000;

/// Timeout de la reponse complete cote demandeur (`socks5.rs`).
pub const HTTP_RESPONSE_TIMEOUT_MS: u64 = 5000;

/// Buffer de lecture de la requete CONNECT (`socks5.rs` : 100 Kio).
pub const CONNECT_REQUEST_MAX: usize = 100 * 1024;

/// `send_tcp_request` (`ipv8-rust-tunnels/src/util.rs`) : envoie les
/// octets de la requete brute sur TCP et relit la reponse complete
/// (headers + corps, avec support `Content-Length` et `chunked`).
/// Retourne les octets filaires complets de la reponse.
pub async fn send_tcp_request(target: &UdpAddress, request: &[u8]) -> Result<Vec<u8>, Ipv8Error> {
    let mut stream = match target {
        UdpAddress::Ipv4(a) => TcpStream::connect(SocketAddr::V4(*a)).await?,
        UdpAddress::Ipv6(a) => TcpStream::connect(SocketAddr::V6(*a)).await?,
        UdpAddress::Domain(host, port) => TcpStream::connect((host.as_str(), *port)).await?,
    };
    stream.write_all(request).await?;

    let mut reader = BufReader::new(stream);
    let mut headers = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = reader.read_until(b'\n', &mut line).await?;
        headers.extend_from_slice(&line);
        // Fin des en-tetes : ligne vide (CRLF ou LF seuls).
        if n < 3 {
            break;
        }
    }

    let headers_text = String::from_utf8_lossy(&headers).to_string();
    let mut chunked = false;
    let mut content_length = 0usize;
    for header in headers_text.split('\n') {
        if let Some(v) = header.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        if header.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }

    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await?;
        headers.extend_from_slice(&body);
        return Ok(headers);
    }

    let mut remainder = Vec::new();
    loop {
        let mut buf = [0u8; 4096];
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => remainder.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => return Err(e.into()),
        }
    }

    if !chunked {
        headers.extend_from_slice(&remainder);
        return Ok(headers);
    }

    // Reassemble le corps `chunked` (chunks hexadecimaux, termines
    // par un chunk de taille 0).
    let mut body = Vec::new();
    let mut rest: &[u8] = &remainder;
    while let Some(pos) = rest.iter().position(|&b| b == b'\n') {
        let size_str = String::from_utf8_lossy(&rest[..pos]);
        let Ok(size) = usize::from_str_radix(size_str.trim(), 16) else {
            break;
        };
        rest = &rest[pos + 1..];
        if size == 0 || rest.len() < size + 2 {
            break;
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..]; // chunk + CRLF
    }
    headers.extend_from_slice(&body);
    Ok(headers)
}
