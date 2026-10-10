// Mesure ADR-0023 etape 84 : le chemin de telechargement (hash RAM
// + write_piece) n'emet AUCUN pread de relecture de verification —
// seuls le check initial et l'upload relisent le disque.

use tempfile::TempDir;

use crate::{
    CreateTorrentOptions, create_torrent,
    file_ops::FileOps,
    spawn_utils::BlockingSpawner,
    storage::filesystem::{FilesystemStorage, OpenedFile},
    tests::test_util::{create_default_random_dir_with_torrents, setup_test_logging},
    torrent_state::TorrentMetadata,
};

#[tokio::test(flavor = "multi_thread")]
async fn io_counters_telechargement_sans_pread() -> anyhow::Result<()> {
    setup_test_logging();

    let piece_length: u32 = 16384;
    let file_size: usize = 16384 + 5000;
    let src = create_default_random_dir_with_torrents(3, file_size, Some("rqbit_io_src"));
    let tf = create_torrent(
        src.path(),
        CreateTorrentOptions {
            piece_length: Some(piece_length),
            ..Default::default()
        },
        &BlockingSpawner::new(1),
    )
    .await?;

    let torrent_bytes = tf.as_bytes()?;
    let metadata = TorrentMetadata::new(
        tf.meta.info.data.validate()?,
        torrent_bytes,
        tf.meta.info.raw_bytes.0.clone(),
    )?;
    let lengths = *metadata.lengths();

    let mut content = Vec::new();
    for fi in metadata.file_infos.iter() {
        content.extend_from_slice(&std::fs::read(src.path().join(&fi.relative_filename))?);
    }

    let out = TempDir::with_prefix("rqbit_io_out")?;
    let storage = FilesystemStorage {
        output_folder: out.path().to_owned(),
        opened_files: metadata
            .file_infos
            .iter()
            .map(|fi| {
                if fi.attrs.padding {
                    OpenedFile::new_dummy()
                } else {
                    OpenedFile::new_lazy(out.path().join(&fi.relative_filename), true)
                }
            })
            .collect(),
        io: Default::default(),
    };
    let fo = FileOps::new(&metadata.info, &storage, &metadata.file_infos);

    // Phase « telechargement » : hash depuis la RAM + ecriture de la
    // piece. Invariant de l'etape 83 : zero pread hors check initial.
    // Les compteurs de `storage.io` sont propres a cette instance —
    // immunises aux E/S des tests paralleles du meme processus.
    storage.io.reset();
    for piece_info in lengths.iter_piece_infos() {
        let idx = piece_info.piece_index;
        let off = lengths.piece_offset(idx) as usize;
        let data = &content[off..off + piece_info.len as usize];
        assert!(fo.check_piece_data(idx, data)?);
        fo.write_piece(idx, data)?;
    }
    let after_download = storage.io.snapshot();
    assert_eq!(
        after_download.pread_ops, 0,
        "pread de relecture pendant le telechargement : {after_download:?}"
    );
    assert_eq!(
        after_download.pread_bytes, 0,
        "octets relus pendant le telechargement : {after_download:?}"
    );
    assert!(
        after_download.pwrite_ops > 0,
        "aucun pwrite mesure : {after_download:?}"
    );
    assert_eq!(
        after_download.pwrite_bytes,
        lengths.total_length(),
        "octets ecrits != taille du torrent : {after_download:?}"
    );

    // Phase « check » (initial_check / re-verification) : les pread
    // sont attendus — c'est le seul lecteur legitime du contenu.
    storage.io.reset();
    let progress = std::sync::atomic::AtomicU64::new(0);
    let pause = std::sync::atomic::AtomicBool::new(false);
    let have = fo.initial_check(&progress, &pause)?;
    assert_eq!(have.count_ones(), lengths.total_pieces() as usize);
    let after_check = storage.io.snapshot();
    assert!(
        after_check.pread_bytes >= lengths.total_length(),
        "check initial devrait relire tout le contenu : {after_check:?}"
    );
    Ok(())
}
