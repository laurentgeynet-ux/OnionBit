//! Verification du patch vendored `UtpAcceptor` : un SYN uTP injecte
//! dans une `UtpSocket` adossee a un transport factice doit etre
//! accepte par `accept()`, et le SYN-ACK doit repartir sur ce meme
//! transport. C'est le comportement dont dependent les lanes anonymes
//! (`TunnelUdpSocket`) : aucune socket UDP reelle n'est ouverte.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use librqbit::UtpAcceptor;
use librqbit_dualstack_sockets::PollSendToVectored;
use librqbit_utp::{DefaultUtpEnvironment, SocketOpts, Transport, UtpSocket};
use tokio::sync::{mpsc, Mutex};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Handles externes du transport factice : `in_tx` simule la
/// reception de datagrammes, `out_rx` capture ce que le socket uTP
/// emet (jamais de socket UDP reelle).
struct FakeHandles {
    in_tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    out_rx: mpsc::Receiver<(Vec<u8>, SocketAddr)>,
}

/// Transport datagramme factice : `recv_from` depile la file
/// d'entree, `send_to`/`poll_send_to*` poussent dans la file de
/// sortie. Equivalent minimal de `TunnelUdpSocket` pour isoler le
/// chemin `accept()` du protocole hidden-service.
struct FakeTransport {
    bind: SocketAddr,
    rx: Mutex<mpsc::Receiver<(Vec<u8>, SocketAddr)>>,
    out_tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
}

impl FakeTransport {
    fn new(bind: SocketAddr) -> (Self, FakeHandles) {
        let (in_tx, in_rx) = mpsc::channel(16);
        let (out_tx, out_rx) = mpsc::channel(16);
        (
            Self {
                bind,
                rx: Mutex::new(in_rx),
                out_tx,
            },
            FakeHandles { in_tx, out_rx },
        )
    }

    fn emit(&self, buf: &[u8], target: SocketAddr) -> usize {
        self.out_tx
            .try_send((buf.to_vec(), target))
            .expect("file de sortie saturee");
        buf.len()
    }
}

impl PollSendToVectored for FakeTransport {
    fn poll_send_to_vectored(
        &self,
        _cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
        target: SocketAddr,
    ) -> Poll<io::Result<usize>> {
        let len: usize = bufs.iter().map(|b| b.len()).sum();
        let mut flat = Vec::with_capacity(len);
        for b in bufs {
            flat.extend_from_slice(b);
        }
        Poll::Ready(Ok(self.emit(&flat, target)))
    }
}

impl Transport for FakeTransport {
    async fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some((data, from)) => {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                Ok((n, from))
            }
            None => std::future::pending().await,
        }
    }

    async fn send_to<'a>(&'a self, buf: &'a [u8], target: SocketAddr) -> io::Result<usize> {
        Ok(self.emit(buf, target))
    }

    fn poll_send_to(
        &self,
        _cx: &mut Context<'_>,
        buf: &[u8],
        target: SocketAddr,
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(self.emit(buf, target)))
    }

    fn bind_addr(&self) -> SocketAddr {
        self.bind
    }
}

/// Datagramme SYN uTP v1 : `ST_SYN << 4 | 1`, ext 0, conn_id et
/// seq_nr arbitraires (le dispatcher n'exige qu'une trame bien
/// formee).
fn syn_packet(conn_id: u16, seq_nr: u16) -> Vec<u8> {
    let mut p = vec![0x41, 0x00];
    p.extend_from_slice(&conn_id.to_be_bytes());
    p.extend_from_slice(&[0; 12]); // ts, ts_diff, wnd_size
    p.extend_from_slice(&seq_nr.to_be_bytes());
    p.extend_from_slice(&[0; 2]); // ack_nr
    p
}

/// Datagramme ST_DATA uTP : premier segment attendu apres le SYN —
/// `conn_id_recv = syn.conn_id + 1`, `seq_nr = syn.seq_nr + 1`,
/// `ack_nr` = seq_nr du SYN-ACK recu - 1 (convention librqbit-utp :
/// l'initiateur fait comme si le SYN-ACK portait `seq_nr - 1`, cf.
/// `StreamArgs::new_outgoing` et la validation `SynAckSent` qui exige
/// `ack_nr == self.seq_nr - 1`).
fn data_packet(conn_id: u16, seq_nr: u16, ack_nr: u16, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![0x01, 0x00];
    p.extend_from_slice(&conn_id.to_be_bytes());
    p.extend_from_slice(&[0; 12]); // ts, ts_diff, wnd_size
    p.extend_from_slice(&seq_nr.to_be_bytes());
    p.extend_from_slice(&ack_nr.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// `accept()` sur une socket adossee a un transport custom doit
/// livrer un `UtpStream` quand un SYN arrive par ce transport, et
/// le SYN-ACK (`ST_STATE`) doit repartir par ce meme transport.
#[tokio::test]
async fn accepteur_utp_accepte_syn_sur_transport_custom() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,librqbit_utp=debug".into()),
        )
        .with_test_writer()
        .try_init();
    let bind: SocketAddr = "127.0.0.1:41024".parse().unwrap();
    let remote: SocketAddr = "203.0.113.7:1024".parse().unwrap();
    let (transport, mut handles) = FakeTransport::new(bind);
    let utp = UtpSocket::new_with_opts(transport, DefaultUtpEnvironment {}, SocketOpts::default())
        .expect("creation socket uTP");

    // Meme chemin que `ListenerOptions::utp_socket` : trait object
    // `UtpAcceptor` partage avec le connector sortant.
    let acceptor: Arc<dyn UtpAcceptor> = utp;
    let accept_task = tokio::spawn(async move { acceptor.accept().await });

    // Le SYN arrive par le transport (cellule `data` e2e cote lane
    // reelle).
    handles
        .in_tx
        .send((syn_packet(0x1234, 0x0100), remote))
        .await
        .unwrap();

    // SYN-ACK : ST_STATE (0x2x) renvoye vers l'expediteur du SYN.
    let (reply, dst) = tokio::time::timeout(TEST_TIMEOUT, handles.out_rx.recv())
        .await
        .expect("aucun SYN-ACK emis")
        .expect("file de sortie fermee");
    assert_eq!(reply[0] >> 4, 2, "type attendu ST_STATE: {reply:02x?}");
    assert_eq!(dst, remote);
    // seq_nr du SYN-ACK = ce que le stream attend comme ack_nr - 1
    // (cf. `StreamArgs::new_outgoing` : `last_consumed_remote_seq_nr =
    // remote_ack.seq_nr - 1` — l'initiateur acquitte le SYN-ACK comme
    // s'il portait le numero precedent).
    let synack_seq = u16::from_be_bytes([reply[16], reply[17]]);

    let mut stream = tokio::time::timeout(TEST_TIMEOUT, accept_task)
        .await
        .expect("accept() bloque")
        .expect("task accept")
        .expect("accept() en erreur");
    assert_eq!(stream.remote_addr(), remote);

    // Le segment suivant (conn_id+1, seq_nr+1) doit devenir lisible
    // sur le stream accepte — c'est le chemin du handshake BitTorrent
    // du downloader sur la lane du seeder.
    use tokio::io::AsyncReadExt;
    handles
        .in_tx
        .send((
            data_packet(
                0x1235,
                0x0101,
                synack_seq.wrapping_sub(1),
                b"bt-handshake-factice",
            ),
            remote,
        ))
        .await
        .unwrap();
    let mut buf = vec![0u8; 64];
    let n = tokio::time::timeout(TEST_TIMEOUT, stream.read(&mut buf))
        .await
        .expect("read() bloque : le DATA n'a pas ete livre au stream")
        .expect("read() en erreur");
    assert_eq!(&buf[..n], b"bt-handshake-factice");
}

/// Environnement deterministe : `random_u16` fixe pour connaitre a
/// l'avance le `seq_nr` du SYN-ACK et construire un DATA valide avant
/// l'acceptation.
#[derive(Clone, Copy)]
struct FixedEnv(u16);

impl librqbit_utp::UtpEnvironment for FixedEnv {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }

    fn copy(&self) -> Self {
        *self
    }

    fn random_u16(&self) -> u16 {
        self.0
    }
}

/// Race observee en live : un ST_DATA qui arrive alors que le SYN est
/// encore en file (`try_cache_syn`, aucun accepteur en attente) tombe
/// sur `streams.get` vide et est perdu silencieusement — le stream cree
/// plus tard reste en `SynAckSent` et le pair meurt ("remote was
/// inactive for too long" apres retransmissions de SYN-ACK).
/// Reproduction : injecter SYN + DATA valide *avant* `accept()`.
#[tokio::test]
async fn accepteur_utp_syn_cache_puis_data() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,librqbit_utp=debug".into()),
        )
        .with_test_writer()
        .try_init();
    const SYNACK_SEQ: u16 = 0x4000;
    let bind: SocketAddr = "127.0.0.1:41025".parse().unwrap();
    let remote: SocketAddr = "203.0.113.8:1024".parse().unwrap();
    let (transport, mut handles) = FakeTransport::new(bind);
    let utp = UtpSocket::new_with_opts(transport, FixedEnv(SYNACK_SEQ), SocketOpts::default())
        .expect("creation socket uTP");

    // SYN puis DATA valide (ack_nr = seq du futur SYN-ACK - 1, cf.
    // `StreamArgs::new_outgoing`) injectes AVANT accept() : le SYN est
    // cache, aucun stream n'existe a l'arrivee du DATA.
    handles
        .in_tx
        .send((syn_packet(0x1234, 0x0100), remote))
        .await
        .unwrap();
    handles
        .in_tx
        .send((
            data_packet(
                0x1235,
                0x0101,
                SYNACK_SEQ.wrapping_sub(1),
                b"bt-handshake-factice",
            ),
            remote,
        ))
        .await
        .unwrap();
    tokio::task::yield_now().await;

    let acceptor: Arc<dyn UtpAcceptor> = utp;
    let accept_task = tokio::spawn(async move { acceptor.accept().await });

    let (reply, _dst) = tokio::time::timeout(TEST_TIMEOUT, handles.out_rx.recv())
        .await
        .expect("aucun SYN-ACK emis")
        .expect("file de sortie fermee");
    assert_eq!(reply[0] >> 4, 2);

    let mut stream = tokio::time::timeout(TEST_TIMEOUT, accept_task)
        .await
        .expect("accept() bloque")
        .expect("task accept")
        .expect("accept() en erreur");

    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 64];
    let n = tokio::time::timeout(TEST_TIMEOUT, stream.read(&mut buf))
        .await
        .expect("read() bloque : le DATA n'a pas ete livre au stream")
        .expect("read() en erreur");
    assert_eq!(&buf[..n], b"bt-handshake-factice");
}
