use std::path::PathBuf;

use anyhow::Context;
use bitvec::{boxed::BitBox, order::Msb0, slice::BitSlice, vec::BitVec};
use tracing::debug_span;

use crate::{spawn_utils::BlockingSpawner, storage::filesystem::OurFileExt};

pub trait BitV: Send + Sync {
    fn as_slice(&self) -> &BitSlice<u8, Msb0>;
    fn as_slice_mut(&mut self) -> &mut BitSlice<u8, Msb0>;
    fn into_dyn(self) -> Box<dyn BitV>;
    fn as_bytes(&self) -> &[u8];
    fn flush(&mut self, flush_async: bool) -> anyhow::Result<()>;
}

pub type BoxBitV = Box<dyn BitV>;

struct DiskFlushRequest {
    snapshot: BitBox<u8, Msb0>,
    /// Accuse de fin d'ecriture+fsync : le flush synchrone (`pause`,
    /// arret session) attend que le snapshot soit reellement sur
    /// disque — sinon le drop-flush asynchrone peut etre lu trop tot
    /// par la restauration suivante (progression perdue).
    ack: Option<std::sync::mpsc::SyncSender<()>>,
}

pub struct DiskBackedBitV {
    bv: BitBox<u8, Msb0>,
    flush_tx: tokio::sync::mpsc::UnboundedSender<DiskFlushRequest>,
}

impl Drop for DiskBackedBitV {
    fn drop(&mut self) {
        if self
            .flush_tx
            .send(DiskFlushRequest {
                snapshot: self.bv.clone(),
                ack: None,
            })
            .is_err()
        {
            tracing::warn!("error flushing bitv on drop: flusher task is dead")
        }
    }
}

// NOTE on mmap. rqbit used it for a while, but it has issues on slow disks.
// We want writes to bitv to be instant in RAM. However when disk is slow, occasionally
// the writes stall which blocks the executor.
// Thus this separate "thread" of flushing was implemented.
impl DiskBackedBitV {
    pub async fn new(filename: PathBuf, spawner: BlockingSpawner) -> anyhow::Result<Self> {
        let buf = tokio::fs::read(&filename)
            .await
            .with_context(|| format!("error reading {filename:?}"))?;
        let bv = BitVec::from_vec(buf).into_boxed_bitslice();

        // blocking file to avoid double-buffering and double-memcpy
        let file = spawner
            .block_in_place_with_semaphore(|| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(false)
                    .open(&filename)
            })
            .await
            .with_context(|| format!("error opening {filename:?}"))?;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DiskFlushRequest>();
        librqbit_core::spawn_utils::spawn(
            debug_span!("diskbitv-flusher", ?filename),
            format!("DiskBackedBitV::flusher {filename:?}"),
            async move {
                loop {
                    let Some(mut req) = rx.recv().await else {
                        break;
                    };
                    let mut acks = Vec::new();
                    if let Some(a) = req.ack.take() {
                        acks.push(a);
                    }
                    while let Ok(mut r) = rx.try_recv() {
                        if let Some(a) = r.ack.take() {
                            acks.push(a);
                        }
                        req = r;
                    }

                    if let Err(e) = spawner
                        .block_in_place_with_semaphore(|| {
                            file.pwrite_all(0, req.snapshot.as_raw_slice())
                        })
                        .await
                    {
                        tracing::error!(?filename, "error writing to bitv: {e:#}");
                        if let Err(e) = tokio::fs::remove_file(&filename).await {
                            tracing::error!(?filename, "error removing bitv: {e:#}");
                        }
                        break;
                    }

                    if let Err(e) = spawner
                        .block_in_place_with_semaphore(|| file.sync_all())
                        .await
                    {
                        tracing::error!(?filename, "error fsyncing bitv: {e:#}");
                    }
                    // Les snapshots fusionnes ci-dessus sont inclus
                    // dans l'ecriture qui vient de se terminer : tous
                    // les attenteurs sont liberes ensemble.
                    for a in acks {
                        let _ = a.send(());
                    }
                }

                Ok::<_, anyhow::Error>(())
            },
        );
        Ok(Self { bv, flush_tx: tx })
    }
}

#[async_trait::async_trait]
impl BitV for BitBox<u8, Msb0> {
    fn as_slice(&self) -> &BitSlice<u8, Msb0> {
        self.as_bitslice()
    }

    fn as_slice_mut(&mut self) -> &mut BitSlice<u8, Msb0> {
        self.as_mut_bitslice()
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_raw_slice()
    }

    fn flush(&mut self, _flush_async: bool) -> anyhow::Result<()> {
        Ok(())
    }

    fn into_dyn(self) -> Box<dyn BitV> {
        Box::new(self)
    }
}

impl BitV for DiskBackedBitV {
    fn as_slice(&self) -> &BitSlice<u8, Msb0> {
        self.bv.as_bitslice()
    }

    fn as_slice_mut(&mut self) -> &mut BitSlice<u8, Msb0> {
        self.bv.as_mut_bitslice()
    }

    fn as_bytes(&self) -> &[u8] {
        self.bv.as_raw_slice()
    }

    fn flush(&mut self, flush_async: bool) -> anyhow::Result<()> {
        if flush_async {
            let req = DiskFlushRequest {
                snapshot: self.bv.clone(),
                ack: None,
            };
            return self.flush_tx.send(req).context("flusher task is dead");
        }
        // Flush synchrone : la requete passe par le meme canal (ordre
        // preserve avec les ecritures en vol) puis on bloque sur
        // l'accuse — appele depuis `pause`, bref et borne. Sur runtime
        // mono-thread le flusher partagerait le fil bloque : on garde
        // le chemin asynchrone (meme comportement qu'avant) plutot
        // qu'un deadlock.
        let multi = matches!(
            tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()),
            Ok(tokio::runtime::RuntimeFlavor::MultiThread)
        );
        if !multi {
            let req = DiskFlushRequest {
                snapshot: self.bv.clone(),
                ack: None,
            };
            return self.flush_tx.send(req).context("flusher task is dead");
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(0);
        self.flush_tx
            .send(DiskFlushRequest {
                snapshot: self.bv.clone(),
                ack: Some(tx),
            })
            .context("flusher task is dead")?;
        // Borne de securite : un flusher sain repond en quelques ms ;
        // au-dela on ne bloque pas la pause plus longtemps.
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .context("flush bitv synchrone en timeout (5s)")
    }

    fn into_dyn(self) -> Box<dyn BitV> {
        Box::new(self)
    }
}

impl BitV for Box<dyn BitV> {
    fn as_slice(&self) -> &BitSlice<u8, Msb0> {
        (**self).as_slice()
    }

    fn as_slice_mut(&mut self) -> &mut BitSlice<u8, Msb0> {
        (**self).as_slice_mut()
    }

    fn as_bytes(&self) -> &[u8] {
        (**self).as_bytes()
    }

    fn flush(&mut self, flush_async: bool) -> anyhow::Result<()> {
        (**self).flush(flush_async)
    }

    fn into_dyn(self) -> Box<dyn BitV> {
        self
    }
}
