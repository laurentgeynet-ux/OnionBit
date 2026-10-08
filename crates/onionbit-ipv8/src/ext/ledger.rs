// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `LedgerLink` — lien bilaterlal signe du ledger de contribution
//! (ADR-0015 §5, Phase 9c : sign-then-serve par tranches).
//!
//! Chaque lien est insere dans les **deux** chaines : `seq_a`/`prev_a`
//! l'ordonnent dans la chaine du serveur `pk_a` (celui qui a servi les
//! octets et propose le lien), `seq_b`/`prev_b` dans celle du
//! beneficiaire `pk_b` (co-signataire). `prev_*` = sha256 du lien
//! precedent de la chaine concernee (`[0; 32]` a la genese) — la
//! validation d'un maillon coute O(1) : verifier les deux signatures
//! + comparer `prev` a la tete de chaine connue.
//!
//! **`tx` chiffre pour la paire** (`pair_seal` : X25519 des deux cles
//! de transport -> ChaCha20-Poly1305) : un tiers qui relaie le lien
//! (gossip des tetes) ne lit ni les volumes ni l'horodatage — le
//! *bandwidth crawler* passif de Tribler 7.x est l'anti-patron
//! documente par l'ADR. Seuls `pk`/`seq`/`prev` restent en clair :
//! c'est exactement ce qu'il faut pour la detection de forks.
//!
//! Pas de consensus : la detection de fork se fait par diffusion des
//! tetes de chaine (liens complets, auto-portants) ; deux liens signes
//! portant la meme cle `(pk, seq)` avec des contenus differents sont
//! une **preuve d'equivocation** — conservee et propagee, jamais
//! tranchee.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use onionbit_crypto::hash::sha256;
use onionbit_crypto::ipv8::dh::{pair_open, pair_seal};
use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};

use crate::error::Ipv8Error;
use crate::serializer::{Reader, Writer};

/// Domaine de signature des liens : separe ces objets de tout autre
/// materiel signe par la meme cle (`hello`, `ATTEST`, messagerie).
const SIG_DOMAIN: &[u8] = b"onionbit/ledger/v1";

/// Version du format de lien (`{v, ...}`) — v1.
pub const LEDGER_VERSION: u8 = 1;

/// Hash de chaine d'un lien : `sha256` de sa forme packee complete.
pub type LinkHash = [u8; 32];

/// `LinkHash` de la genese (`prev_*` d'un premier lien).
pub const GENESIS: LinkHash = [0u8; 32];

/// Contenu signe d'un `tx` — lisible uniquement par les deux parties
/// (payload chiffre pour la paire) : le total d'octets que `pk_a`
/// declare avoir servi a `pk_b` depuis la genese, et son horodatage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerTx {
    /// Version du format `tx` (1).
    pub version: u8,
    /// Octets servis par `pk_a` pour `pk_b` (cumulatif — le dernier
    /// lien d'une paire contient le total courant).
    pub served_total: u64,
    /// Horodatage createur (secondes Unix).
    pub ts: u64,
}

impl LedgerTx {
    /// Cree le contenu `tx` v1.
    pub fn new(served_total: u64, ts: u64) -> Self {
        Self {
            version: LEDGER_VERSION,
            served_total,
            ts,
        }
    }

    /// Forme packee du `tx` (clair interne, avant chiffrement).
    fn pack(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.version);
        w.u64(self.served_total);
        w.u64(self.ts);
        w.into_bytes()
    }

    /// Deserialise le `tx` clair (borne — `Reader` refuse les
    /// troncatures).
    fn unpack(r: &mut Reader) -> Result<Self, Ipv8Error> {
        let tx = Self {
            version: r.u8()?,
            served_total: r.u64()?,
            ts: r.u64()?,
        };
        if tx.version != LEDGER_VERSION {
            return Err(Ipv8Error::Malformed("version de tx ledger inconnue"));
        }
        Ok(tx)
    }

    /// Chiffre le `tx` pour la paire (`my_sk` = cle X25519 secrete de
    /// l'emetteur, `peer_pk` = cle X25519 publique du co-signataire).
    pub fn seal(
        &self,
        my_key: &LibNaClSecretKey,
        peer_pk: &LibNaClPublicKey,
    ) -> Result<Vec<u8>, Ipv8Error> {
        pair_seal(
            &peer_pk.crypt_pk,
            &my_key.crypt_x25519().to_bytes(),
            &self.pack(),
        )
        .map_err(Ipv8Error::Crypto)
    }

    /// Dechiffre un `tx_enc` recu — seul un membre de la paire peut.
    /// `my_key` = notre cle secrete, `peer_pk` = cle publique de
    /// l'autre partie (emetteur ou co-signataire indifferemment).
    pub fn open(
        tx_enc: &[u8],
        my_key: &LibNaClSecretKey,
        peer_pk: &LibNaClPublicKey,
    ) -> Result<Self, Ipv8Error> {
        let plain = pair_open(&peer_pk.crypt_pk, &my_key.crypt_x25519().to_bytes(), tx_enc)
            .map_err(Ipv8Error::Crypto)?;
        let mut r = Reader::new(&plain);
        Self::unpack(&mut r)
    }
}

/// Lien bilateral signe : `pk_a` a propose (premiere signature), `pk_b`
/// a co-signe (`sig_b` — `[0; 64]` dans une proposition pas encore
/// scellee).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerLink {
    /// Version du format (`LEDGER_VERSION`).
    pub version: u8,
    /// Cle publique du serveur (propose + signe en premier).
    pub pk_a: Vec<u8>,
    /// Rang du lien dans la chaine de `pk_a` (genese = 1).
    pub seq_a: u64,
    /// Hash du lien precedent de la chaine de `pk_a` ([0;32] genese).
    pub prev_a: LinkHash,
    /// Cle publique du beneficiaire (co-signataire).
    pub pk_b: Vec<u8>,
    /// Rang du lien dans la chaine de `pk_b`.
    pub seq_b: u64,
    /// Hash du lien precedent de la chaine de `pk_b`.
    pub prev_b: LinkHash,
    /// `tx` chiffre pour la paire (`LedgerTx::seal`).
    pub tx_enc: Vec<u8>,
    /// Signature Ed25519 de `pk_a` sur les champs signes.
    pub sig_a: [u8; 64],
    /// Signature Ed25519 de `pk_b` — `[0; 64]` tant que la
    /// proposition n'est pas co-signee.
    pub sig_b: [u8; 64],
}

impl LedgerLink {
    /// Construit une proposition signee par `key_a` (le serveur) :
    /// champs des deux chaines + `tx` chiffre pour la paire, `sig_b`
    /// vide. `seq_b`/`prev_b` sont la **vue** qu'a `pk_a` de la tete
    /// de `pk_b` — le co-signataire la verifie contre sa propre
    /// chaine et rejette si elle est perimee.
    #[allow(clippy::too_many_arguments)]
    pub fn propose(
        key_a: &LibNaClSecretKey,
        pk_b: &LibNaClPublicKey,
        seq_a: u64,
        prev_a: LinkHash,
        seq_b: u64,
        prev_b: LinkHash,
        tx: &LedgerTx,
    ) -> Result<Self, Ipv8Error> {
        let mut link = Self {
            version: LEDGER_VERSION,
            pk_a: key_a.public_key().to_bin(),
            seq_a,
            prev_a,
            pk_b: pk_b.to_bin(),
            seq_b,
            prev_b,
            tx_enc: tx.seal(key_a, pk_b)?,
            sig_a: [0; 64],
            sig_b: [0; 64],
        };
        link.check_shape(false)?;
        link.sig_a = key_a.sign(&link.signed_bytes());
        Ok(link)
    }

    /// Co-signe une proposition recue (cote `pk_b`) : pose `sig_b`
    /// apres verification de `sig_a` et de la coherence avec la
    /// chaine locale faite par l'appelant.
    pub fn cosign(&mut self, key_b: &LibNaClSecretKey) -> Result<(), Ipv8Error> {
        if self.pk_b != key_b.public_key().to_bin() {
            return Err(Ipv8Error::Malformed(
                "cosignature par une autre cle que pk_b",
            ));
        }
        self.sig_b = key_b.sign(&self.signed_bytes());
        Ok(())
    }

    /// Octets couverts par les deux signatures :
    /// `SIG_DOMAIN || {v, pk_a, seq_a, prev_a, pk_b, seq_b, prev_b,
    /// tx_enc}` — une signature est verifiable independamment de
    /// l'autre.
    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SIG_DOMAIN.len() + 220);
        out.extend_from_slice(SIG_DOMAIN);
        let mut w = Writer::new();
        self.pack_fields(&mut w);
        out.extend_from_slice(&w.into_bytes());
        out
    }

    /// Champs signes : tout sauf les signatures.
    fn pack_fields(&self, w: &mut Writer) {
        w.u8(self.version);
        w.varlen_h(&self.pk_a);
        w.u64(self.seq_a);
        w.raw(&self.prev_a);
        w.varlen_h(&self.pk_b);
        w.u64(self.seq_b);
        w.raw(&self.prev_b);
        w.varlen_h(&self.tx_enc);
    }

    /// Forme minimale : version v1, deux `LibNaClPK`, `tx_enc` non
    /// vide, signatures presentes selon `sealed` (sig_a toujours ;
    /// sig_b exigee si `sealed`).
    fn check_shape(&self, sealed: bool) -> Result<(), Ipv8Error> {
        if self.version != LEDGER_VERSION {
            return Err(Ipv8Error::Malformed("version de lien inconnue"));
        }
        LibNaClPublicKey::from_bin(&self.pk_a)
            .map_err(|_| Ipv8Error::Malformed("pk_a n'est pas une LibNaClPK"))?;
        LibNaClPublicKey::from_bin(&self.pk_b)
            .map_err(|_| Ipv8Error::Malformed("pk_b n'est pas une LibNaClPK"))?;
        if self.pk_a == self.pk_b {
            return Err(Ipv8Error::Malformed("lien ledger avec soi-meme"));
        }
        if self.seq_a == 0 || self.seq_b == 0 {
            return Err(Ipv8Error::Malformed("seq de lien nul (genese = 1)"));
        }
        if self.tx_enc.is_empty() {
            return Err(Ipv8Error::Malformed("tx_enc vide"));
        }
        if sealed && self.sig_b == [0; 64] {
            return Err(Ipv8Error::Malformed("lien non scelle (sig_b vide)"));
        }
        Ok(())
    }

    /// `sig_a` valide ?
    pub fn verify_a(&self) -> bool {
        let Ok(pk) = LibNaClPublicKey::from_bin(&self.pk_a) else {
            return false;
        };
        pk.verify(&self.signed_bytes(), &self.sig_a)
    }

    /// `sig_b` valide ? (`false` si signature vide.)
    pub fn verify_b(&self) -> bool {
        if self.sig_b == [0; 64] {
            return false;
        }
        let Ok(pk) = LibNaClPublicKey::from_bin(&self.pk_b) else {
            return false;
        };
        pk.verify(&self.signed_bytes(), &self.sig_b)
    }

    /// Hash de chaine du lien : `sha256` de la forme packee — c'est
    /// ce que les `prev_*` des maillons suivants referencent.
    pub fn hash(&self) -> LinkHash {
        sha256(&self.pack())
    }

    /// Identite de la proposition : `sha256(champs signes || sig_a)`
    /// — **independante de `sig_b`** : la proposition (`sig_b` vide)
    /// et son sceau (signe) partagent le meme `proposal_id`. C'est
    /// la cle de dedup du store et l'identifiant de position pour la
    /// detection de fork (deux liens a la meme position comparent
    /// les `proposal_id`, pas le hash complet — sinon le sceau d'une
    /// proposition stockee serait un faux fork).
    pub fn proposal_id(&self) -> LinkHash {
        let mut buf = self.signed_bytes();
        buf.extend_from_slice(&self.sig_a);
        sha256(&buf)
    }

    /// `sig_b` posee (lien scelle) ?
    pub fn is_sealed(&self) -> bool {
        self.sig_b != [0; 64]
    }

    /// Forme filaire complete : champs + `sig_a` + `sig_b`.
    pub fn pack(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.pack_fields(&mut w);
        w.raw(&self.sig_a);
        w.raw(&self.sig_b);
        w.into_bytes()
    }

    /// Deserialise et controle la **forme** ; `sealed` exige `sig_b`
    /// non nulle. Les signatures restent a verifier par l'appelant
    /// (`verify_a`/`verify_b`) — les deux etapes sont separees pour
    /// fuzzer `unpack` seul. Trailing tolere.
    pub fn unpack(r: &mut Reader, sealed: bool) -> Result<Self, Ipv8Error> {
        let link = Self {
            version: r.u8()?,
            pk_a: r.varlen_h()?.to_vec(),
            seq_a: r.u64()?,
            prev_a: {
                let mut h = [0u8; 32];
                h.copy_from_slice(r.take(32)?);
                h
            },
            pk_b: r.varlen_h()?.to_vec(),
            seq_b: r.u64()?,
            prev_b: {
                let mut h = [0u8; 32];
                h.copy_from_slice(r.take(32)?);
                h
            },
            tx_enc: r.varlen_h()?.to_vec(),
            sig_a: {
                let mut s = [0u8; 64];
                s.copy_from_slice(r.take(64)?);
                s
            },
            sig_b: {
                let mut s = [0u8; 64];
                s.copy_from_slice(r.take(64)?);
                s
            },
        };
        link.check_shape(sealed)?;
        Ok(link)
    }

    /// `mid` hex d'une cle (affichage API / logs).
    pub fn mid_of(pk: &[u8]) -> String {
        hex::encode(onionbit_crypto::hash::ipv8_mid(pk))
    }
}

/// Tete de chaine connue d'un pair : `(seq, hash)` du dernier lien
/// observe le concernant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainHead {
    /// Dernier `seq` connu (0 = inconnu).
    pub seq: u64,
    /// Hash du lien a ce rang ([0;32] = genese attendue).
    pub hash: LinkHash,
}

/// Resultat d'un `put` dans le store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutOutcome {
    /// Lien nouveau et plus recent que la tete connue.
    New,
    /// Deja stocke (meme contenu) — rejeu absorbe.
    Duplicate,
    /// Lien correct mais plus ancien que la tete (stocke pour la
    /// reconstruction, ne fait pas avancer la tete).
    BehindHead,
    /// **Preuve de fork** : un lien different existe deja pour la
    /// meme cle `(pk, seq)` — le lien EST stocke (conservation de la
    /// preuve) mais la tete n'avance pas.
    Fork,
}

/// Persistance des liens verifies (trait injecte — pattern
/// `AttestationStore`/`PeerStatsStore`).
pub trait LedgerStore: Send + Sync {
    /// Persiste un lien completement verifie (deux signatures).
    /// Fork : pour chaque chaine touchee `(pk, seq)`, si un lien
    /// different existe deja → `Fork` (le lien est quand meme stocke
    /// comme preuve).
    fn put(&self, link: &LedgerLink) -> PutOutcome;
    /// Lien stocke a la position `(pk, seq)` d'une chaine.
    fn link_at(&self, pk: &[u8], seq: u64) -> Option<LedgerLink>;
    /// Tete connue de la chaine de `pk` (`(seq max, hash)`).
    fn head(&self, pk: &[u8]) -> Option<ChainHead>;
    /// Liens dont `pk` est partie (ses chaines `a` et `b`
    /// confondues), tries par rang propre a `pk`.
    fn links_of(&self, pk: &[u8], limit: usize) -> Vec<LedgerLink>;
    /// Les `limit` liens les plus recents (horodatage de stockage).
    fn latest(&self, limit: usize) -> Vec<LedgerLink>;
    /// Nombre de liens stockes.
    fn count(&self) -> usize;
}

/// Store memoire borne (tests, defaut sans persistance).
/// Index des positions de chaine : `(pk, seq)` -> identites des
/// liens stockes a cette position (plusieurs = fork avere).
type PositionIndex = HashMap<(Vec<u8>, u64), Vec<LinkHash>>;

pub struct InMemoryLedgerStore {
    /// Liens par hash (dedup par contenu signe).
    links: Mutex<HashMap<LinkHash, StoredLink>>,
    /// Index `(pk, seq) -> Vec<proposal_id>` : un seul id = chaine
    /// coherente, plusieurs = fork avere (preuves conservees).
    by_pos: Mutex<PositionIndex>,
    /// Capacite max — eviction la plus ancienne au-dela.
    max: usize,
}

/// Lien stocke + horodatage d'insertion (ordre de `latest`, eviction).
struct StoredLink {
    link: LedgerLink,
    stored_at: Instant,
}

impl InMemoryLedgerStore {
    /// Store borne a `max` liens ; l'eviction touche le plus ancien
    /// (fond de chaine — les preuves recentes, forks inclus, sont les
    /// plus a jour).
    pub fn new(max: usize) -> Self {
        Self {
            links: Mutex::new(HashMap::new()),
            by_pos: Mutex::new(HashMap::new()),
            max: max.max(1),
        }
    }

    /// Evince les liens les plus anciens jusqu'a `max`.
    fn evict(&self) {
        let mut links = self.links.lock().unwrap();
        if links.len() <= self.max {
            return;
        }
        let mut by_pos = self.by_pos.lock().unwrap();
        // Tri par insertion : les `cut` plus anciens partent.
        let mut order: Vec<(Instant, LinkHash)> =
            links.iter().map(|(h, s)| (s.stored_at, *h)).collect();
        order.sort();
        for (_, h) in order.into_iter().take(links.len() - self.max) {
            if let Some(s) = links.remove(&h) {
                // Meme regle qu'a l'insertion : une proposition non
                // scellee n'a jamais occupe la position `(pk_b, seq_b)`.
                let n = 1 + usize::from(s.link.is_sealed());
                for (pk, seq) in [(s.link.pk_a, s.link.seq_a), (s.link.pk_b, s.link.seq_b)]
                    .into_iter()
                    .take(n)
                {
                    let key = (pk, seq);
                    let empty = if let Some(hashes) = by_pos.get_mut(&key) {
                        hashes.retain(|x| *x != h);
                        hashes.is_empty()
                    } else {
                        false
                    };
                    if empty {
                        by_pos.remove(&key);
                    }
                }
            }
        }
    }

    /// Enregistre un lien deja valide. Le lien est identifie par
    /// `proposal_id` : un sceau remplace la proposition non scellee
    /// de meme identite (pas un fork). Le fork se detecte sur la
    /// position `(pk, seq)` : un `proposal_id` different y est une
    /// equivocation averee.
    fn store(&self, link: &LedgerLink) -> PutOutcome {
        let pid = link.proposal_id();
        {
            let links = self.links.lock().unwrap();
            if let Some(old) = links.get(&pid) {
                // Meme proposition : seul un passage non-scelle ->
                // scelle est un upgrade (le sceau enrichit la
                // preuve). Tout le reste est un rejeu absorbe.
                if old.link.is_sealed() || !link.is_sealed() {
                    return PutOutcome::Duplicate;
                }
            }
        }
        let mut by_pos = self.by_pos.lock().unwrap();
        // Positions occupees : `(pk_a, seq_a)` toujours — la chaine du
        // proposeur avance des l'emission, un second lien a ce `seq_a`
        // est une equivocation signee par `a`. `(pk_b, seq_b)` seulement
        // une fois `sig_b` posee : une proposition non scellee n'engage
        // pas la position du co-signataire — sinon un REJECT suivi
        // d'une reproposition au meme `seq_b` s'auto-marquerait fork
        // (observe au banc `bench_ext_ledger_soak` : forks=2 fantomes).
        let n = 1 + usize::from(link.is_sealed());
        let mut fork = false;
        for (pk, seq) in [(&link.pk_a, link.seq_a), (&link.pk_b, link.seq_b)]
            .into_iter()
            .take(n)
        {
            let ids = by_pos.entry((pk.clone(), seq)).or_default();
            if ids.iter().any(|id| *id != pid) {
                fork = true;
            }
            if !ids.contains(&pid) {
                ids.push(pid);
            }
        }
        // Un lien plus ancien que la tete connue de l'une de ses
        // chaines n'avance pas les tetes.
        let behind = [(&link.pk_a, link.seq_a), (&link.pk_b, link.seq_b)]
            .iter()
            .take(n)
            .any(|(pk, seq)| {
                by_pos
                    .keys()
                    .filter(|(p, _)| p == *pk)
                    .map(|(_, s)| *s)
                    .max()
                    .is_some_and(|max| *seq < max)
            });
        drop(by_pos);
        self.links.lock().unwrap().insert(
            pid,
            StoredLink {
                link: link.clone(),
                stored_at: Instant::now(),
            },
        );
        self.evict();
        if fork {
            PutOutcome::Fork
        } else if behind {
            PutOutcome::BehindHead
        } else {
            PutOutcome::New
        }
    }

    /// Tete d'une chaine dans les maps verrouillees : le lien au plus
    /// grand `seq` (premier hash si fork au sommet).
    fn head_locked(
        by_pos: &PositionIndex,
        links: &HashMap<LinkHash, StoredLink>,
        pk: &[u8],
    ) -> Option<ChainHead> {
        let seq = by_pos
            .keys()
            .filter(|(p, _)| p == pk)
            .map(|(_, s)| *s)
            .max()?;
        // `by_pos` indexe par `proposal_id` ; `ChainHead.hash` porte
        // le **hash de chaine** du lien (ce que `prev_*` pointe).
        let pid = *by_pos.get(&(pk.to_vec(), seq))?.first()?;
        let s = links.get(&pid)?;
        Some(ChainHead {
            seq,
            hash: s.link.hash(),
        })
    }
}

impl LedgerStore for InMemoryLedgerStore {
    fn put(&self, link: &LedgerLink) -> PutOutcome {
        self.store(link)
    }

    fn link_at(&self, pk: &[u8], seq: u64) -> Option<LedgerLink> {
        // Ordre de verrouillage impose `links` puis `by_pos` — le
        // meme que `store`/`evict` (interdit l'inter-ordre ABBA).
        let links = self.links.lock().unwrap();
        let by_pos = self.by_pos.lock().unwrap();
        let pid = *by_pos.get(&(pk.to_vec(), seq))?.first()?;
        links.get(&pid).map(|s| s.link.clone())
    }

    fn head(&self, pk: &[u8]) -> Option<ChainHead> {
        let links = self.links.lock().unwrap();
        let by_pos = self.by_pos.lock().unwrap();
        Self::head_locked(&by_pos, &links, pk)
    }

    fn links_of(&self, pk: &[u8], limit: usize) -> Vec<LedgerLink> {
        let links = self.links.lock().unwrap();
        let mut v: Vec<LedgerLink> = links
            .values()
            .filter(|s| s.link.pk_a == pk || s.link.pk_b == pk)
            .map(|s| s.link.clone())
            .collect();
        // Rang dans la chaine propre a `pk`.
        v.sort_by_key(|l| if l.pk_a == pk { l.seq_a } else { l.seq_b });
        v.truncate(limit);
        v
    }

    fn latest(&self, limit: usize) -> Vec<LedgerLink> {
        let links = self.links.lock().unwrap();
        let mut v: Vec<(&Instant, &LedgerLink)> =
            links.values().map(|s| (&s.stored_at, &s.link)).collect();
        v.sort_by_key(|(t, _)| std::cmp::Reverse(**t));
        v.into_iter().take(limit).map(|(_, l)| l.clone()).collect()
    }

    fn count(&self) -> usize {
        self.links.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Genese + co-signature : round-trip complet a la main.
    fn sealed_pair() -> (LibNaClSecretKey, LibNaClSecretKey, LedgerLink) {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let tx = LedgerTx::new(1024, now());
        let mut link =
            LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx).unwrap();
        link.cosign(&b).unwrap();
        (a, b, link)
    }

    #[test]
    fn propose_cosign_verify_roundtrip() {
        let (_a, _b, link) = sealed_pair();
        assert!(link.verify_a());
        assert!(link.verify_b());
        let packed = link.pack();
        let mut r = Reader::new(&packed);
        let got = LedgerLink::unpack(&mut r, true).expect("unpack");
        assert_eq!(got, link);
        assert!(got.verify_a() && got.verify_b());
    }

    #[test]
    fn tx_lisible_par_la_paire_seule() {
        let (a, b, link) = sealed_pair();
        // Les deux parties dechiffrent le tx.
        let pk_b = b.public_key();
        let tx_b = LedgerTx::open(&link.tx_enc, &b, &a.public_key()).unwrap();
        let tx_a = LedgerTx::open(&link.tx_enc, &a, &pk_b).unwrap();
        assert_eq!(tx_a, tx_b);
        assert_eq!(tx_a.served_total, 1024);
        // Un tiers ne peut pas.
        let c = LibNaClSecretKey::generate();
        assert!(LedgerTx::open(&link.tx_enc, &c, &a.public_key()).is_err());
    }

    #[test]
    fn signature_sur_domaine_separe() {
        let key = LibNaClSecretKey::generate();
        let other = LibNaClSecretKey::generate();
        let tx = LedgerTx::new(1, 1);
        let link =
            LedgerLink::propose(&key, &other.public_key(), 1, GENESIS, 1, GENESIS, &tx).unwrap();
        // Sans le domaine, la signature ne verifie pas.
        let mut w = Writer::new();
        link.pack_fields(&mut w);
        let pk = LibNaClPublicKey::from_bin(&link.pk_a).unwrap();
        assert!(!pk.verify(&w.into_bytes(), &link.sig_a));
    }

    #[test]
    fn formes_rejetees() {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let tx = LedgerTx::new(10, 10);
        // seq 0 = refuse.
        assert!(LedgerLink::propose(&a, &b.public_key(), 0, GENESIS, 1, GENESIS, &tx).is_err());
        // Soi-meme = refuse.
        assert!(LedgerLink::propose(&a, &a.public_key(), 1, GENESIS, 1, GENESIS, &tx).is_err());
        // Troncatures : erreur, pas de panic.
        let mut link =
            LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx).unwrap();
        link.cosign(&b).unwrap();
        let packed = link.pack();
        for n in [0, 1, 10, 50, 120, packed.len() - 1] {
            let mut r = Reader::new(&packed[..n]);
            assert!(LedgerLink::unpack(&mut r, true).is_err());
        }
        // Unpacked sans sig_b = proposition OK, scellee refusee.
        let prop = LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx).unwrap();
        let packed = prop.pack();
        let mut r = Reader::new(&packed);
        assert!(LedgerLink::unpack(&mut r, false).is_ok());
        let mut r = Reader::new(&packed);
        assert!(LedgerLink::unpack(&mut r, true).is_err());
    }

    #[test]
    fn store_heads_et_fork() {
        let (a, b, link) = sealed_pair();
        let store = InMemoryLedgerStore::new(64);
        assert_eq!(store.put(&link), PutOutcome::New);
        assert_eq!(store.put(&link), PutOutcome::Duplicate);
        let head_a = store.head(&link.pk_a).unwrap();
        assert_eq!((head_a.seq, head_a.hash), (1, link.hash()));
        assert_eq!(store.count(), 1);

        // Deuxieme lien legitime (chaine avance).
        let tx2 = LedgerTx::new(2048, now() + 1);
        let mut l2 =
            LedgerLink::propose(&a, &b.public_key(), 2, link.hash(), 2, link.hash(), &tx2).unwrap();
        l2.cosign(&b).unwrap();
        assert_eq!(store.put(&l2), PutOutcome::New);
        assert_eq!(store.head(&link.pk_a).unwrap().seq, 2);

        // Fork : meme (pk_a, seq=2), autre contenu (served_total
        // different => tx_enc != => sig_a != => hash !=).
        let tx_fork = LedgerTx::new(9999, now() + 1);
        let mut lf = LedgerLink::propose(
            &a,
            &b.public_key(),
            2,
            link.hash(),
            2,
            link.hash(),
            &tx_fork,
        )
        .unwrap();
        lf.cosign(&b).unwrap();
        assert_eq!(store.put(&lf), PutOutcome::Fork);
        // Les deux sont conserves (preuve).
        assert_eq!(store.count(), 3);
    }

    #[test]
    fn proposition_rejetee_puis_reproposee_nest_pas_un_fork() {
        // Banc reel (`bench_ext_ledger_soak`) : `a` propose seq_b=1,
        // `b` refuse (derive), `a` resynchronise et repropose seq_b=1
        // a `seq_a` suivant. La proposition non scellee n'occupait
        // jamais reellement la position de `b` — aucun fork.
        let (a, b, _) = sealed_pair();
        let store = InMemoryLedgerStore::new(64);
        let tx1 = LedgerTx::new(1024, now());
        let p1 = LedgerLink::propose(&a, &b.public_key(), 1, GENESIS, 1, GENESIS, &tx1).unwrap();
        assert_eq!(store.put(&p1), PutOutcome::New); // non scellee
        let tx2 = LedgerTx::new(2048, now() + 1);
        let mut p2 =
            LedgerLink::propose(&a, &b.public_key(), 2, p1.hash(), 1, GENESIS, &tx2).unwrap();
        p2.cosign(&b).unwrap();
        assert_eq!(store.put(&p2), PutOutcome::New); // scellee, meme seq_b

        // Une seconde proposition SCELLEE differente au meme seq_b :
        // la, vraie equivocation de `b` — fork avere.
        let tx3 = LedgerTx::new(4096, now() + 2);
        let mut p3 =
            LedgerLink::propose(&a, &b.public_key(), 3, p2.hash(), 1, GENESIS, &tx3).unwrap();
        p3.cosign(&b).unwrap();
        assert_eq!(store.put(&p3), PutOutcome::Fork);
    }

    #[test]
    fn behind_head_classe_les_liens_anciens() {
        let (a, b, link1) = sealed_pair();
        let store = InMemoryLedgerStore::new(64);
        store.put(&link1);
        let tx2 = LedgerTx::new(2048, now() + 1);
        let mut l2 =
            LedgerLink::propose(&a, &b.public_key(), 2, link1.hash(), 2, link1.hash(), &tx2)
                .unwrap();
        l2.cosign(&b).unwrap();
        let tx3 = LedgerTx::new(4096, now() + 2);
        let mut l3 =
            LedgerLink::propose(&a, &b.public_key(), 3, l2.hash(), 3, l2.hash(), &tx3).unwrap();
        l3.cosign(&b).unwrap();
        // On insere le 3 avant le 2 : le 2 arrive derriere la tete.
        assert_eq!(store.put(&l3), PutOutcome::New);
        assert_eq!(store.put(&l2), PutOutcome::BehindHead);
    }
}
