// Teste la chaîne ADR-0023 au niveau FileOps : hash d'une piece
// assemblee en RAM (check_piece_data), ecriture d'un tenant
// (write_piece), puis re-verification depuis le disque
// (check_piece / initial_check). Couvre les pieces a cheval sur
// plusieurs fichiers (file_size non multiple de piece_length).

use std::sync::atomic::{AtomicBool, AtomicU64};

use anyhow::Context;
use tempfile::TempDir;
use tracing::info;

use crate::{
    CreateTorrentOptions, create_torrent,
    file_ops::FileOps,
    spawn_utils::BlockingSpawner,
    storage::filesystem::{FilesystemStorage, OpenedFile},
    tests::test_util::{create_default_random_dir_with_torrents, setup_test_logging},
    torrent_state::TorrentMetadata,
};

#[tokio::test(flavor = "multi_thread")]
async fn staged_piece_memory_hash_and_single_write() -> anyhow::Result<()> {
    setup_test_logging();

    let piece_length: u32 = 16384;
    let file_size: usize = 16384 * 2 + 5000; // pieces a cheval sur 2 fichiers
    let num_files: usize = 4;

    let src =
        create_default_random_dir_with_torrents(num_files, file_size, Some("rqbit_staged_src"));
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

    // Contenu de reference : concatenation des fichiers dans l'ordre du torrent.
    let mut content = Vec::new();
    for fi in metadata.file_infos.iter() {
        content.extend_from_slice(
            &std::fs::read(src.path().join(&fi.relative_filename))
                .with_context(|| format!("reading {:?}", fi.relative_filename))?,
        );
    }
    assert_eq!(content.len() as u64, lengths.total_length());

    let out = TempDir::with_prefix("rqbit_staged_out")?;
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

    // 1. Hash depuis la RAM puis ecriture, piece par piece.
    for piece_info in lengths.iter_piece_infos() {
        let idx = piece_info.piece_index;
        let off = lengths.piece_offset(idx) as usize;
        let data = &content[off..off + piece_info.len as usize];

        assert!(
            fo.check_piece_data(idx, data)?,
            "check_piece_data a refuse la piece {idx}"
        );
        fo.write_piece(idx, data)?;
    }

    // 2. Relecture disque : check_piece (chemin historique) valide ce
    //    que write_piece a pose — preuve que le layout est identique.
    for piece_info in lengths.iter_piece_infos() {
        assert!(
            fo.check_piece(piece_info.piece_index)?,
            "check_piece disque a echoue pour {}",
            piece_info.piece_index
        );
    }

    // 3. initial_check complet : toutes les pieces doivent etre "have".
    let progress = AtomicU64::new(0);
    let pause = AtomicBool::new(false);
    let have = fo.initial_check(&progress, &pause)?;
    assert_eq!(have.count_ones(), lengths.total_pieces() as usize);

    // 4. Corruption : un octet modifie doit echouer au hash RAM (et ne
    //    doit donc pas etre ecrit — write_piece n'est appele qu'apres
    //    un hash_ok dans le chemin live).
    let first = lengths.iter_piece_infos().next().context("no pieces")?;
    let off = lengths.piece_offset(first.piece_index) as usize;
    let mut bad = content[off..off + first.len as usize].to_vec();
    bad[0] ^= 0xff;
    assert_eq!(fo.check_piece_data(first.piece_index, &bad)?, false);

    // 5. Garde-fou : taille de buffer != taille de piece -> erreur.
    assert!(
        fo.check_piece_data(first.piece_index, &bad[..bad.len() - 1])
            .is_err()
    );

    info!(
        "staged piece round-trip OK, {} pieces",
        lengths.total_pieces()
    );
    Ok(())
}
