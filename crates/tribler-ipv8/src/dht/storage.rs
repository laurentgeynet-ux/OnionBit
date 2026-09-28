//! Stockage DHT en memoire (equivalent de `dht/storage.py`) :
//! `Storage` indexe des valeurs serialisees par cle de 20 octets, avec
//! expiration (`max_age`) et versioning.

use std::collections::HashMap;

/// Duree de vie par defaut d'une entree (`max_age` Python : 86400 s).
pub const DEFAULT_MAX_AGE: f64 = 86400.0;

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `Value` : une valeur DHT stockee.
#[derive(Debug, Clone)]
pub struct Value {
    /// `id` : sha1(pubkey) pour les valeurs signees, sha1(data) sinon.
    pub id: Vec<u8>,
    /// Donnees serialisees (blob `ez_pack`).
    pub data: Vec<u8>,
    /// Horodatage de la derniere ecriture.
    pub last_update: f64,
    /// Duree de vie maximale (secondes).
    pub max_age: f64,
    /// Version (horodatage d'emission pour les valeurs signees).
    pub version: u32,
}

impl Value {
    /// `expired`.
    pub fn expired(&self) -> bool {
        now() - self.last_update > self.max_age
    }
}

/// `Storage` : table cle -> liste de valeurs (port fidele du Python :
/// insertion en tete, tri stable `id == key` en dernier, remplacement
/// si meme `id` et `version >=`).
#[derive(Default)]
pub struct Storage {
    items: HashMap<Vec<u8>, Vec<Value>>,
}

impl Storage {
    /// `put`.
    pub fn put(&mut self, key: &[u8], data: &[u8], id: Vec<u8>, max_age: f64, version: u32) {
        let v = Value {
            id,
            data: data.to_vec(),
            last_update: now(),
            max_age,
            version,
        };
        let list = self.items.entry(key.to_vec()).or_default();
        if let Some(pos) = list.iter().position(|e| e.id == v.id) {
            if v.version >= list[pos].version {
                list.remove(pos);
                list.insert(0, v);
            }
        } else {
            list.insert(0, v);
        }
        // Tri stable : les valeurs dont l'id == key passent en dernier.
        list.sort_by_key(|e| usize::from(&e.id[..] == key));
    }

    /// `get` : `limit` valeurs a partir de `starting_point`.
    pub fn get(&self, key: &[u8], starting_point: usize, limit: Option<usize>) -> Vec<Vec<u8>> {
        let Some(list) = self.items.get(key) else {
            return Vec::new();
        };
        let upper = limit.map(|l| (starting_point + l).min(list.len()));
        let slice = match upper {
            Some(u) if starting_point < u => &list[starting_point..u],
            Some(_) => return Vec::new(),
            None if starting_point < list.len() => &list[starting_point..],
            None => return Vec::new(),
        };
        slice.iter().map(|v| v.data.clone()).collect()
    }

    /// `items_older_than`.
    pub fn items_older_than(&self, min_age: f64) -> Vec<(Vec<u8>, Vec<u8>)> {
        let t = now();
        let mut out = Vec::new();
        for (k, vs) in &self.items {
            for v in vs {
                if t - v.last_update > min_age {
                    out.push((k.clone(), v.data.clone()));
                }
            }
        }
        out
    }

    /// `clean` : retire les valeurs expirees (en fin de liste, comme le
    /// Python qui itere a rebours).
    pub fn clean(&mut self) {
        for list in self.items.values_mut() {
            while let Some(v) = list.last() {
                if v.expired() {
                    list.pop();
                } else {
                    break;
                }
            }
        }
        self.items.retain(|_, vs| !vs.is_empty());
    }

    /// Instantane des donnees brutes non expirees par cle —
    /// `storage.items` de `get_stored_values` Python.
    pub fn items_snapshot(&self) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
        self.items
            .iter()
            .map(|(k, vs)| {
                (
                    k.clone(),
                    vs.iter()
                        .filter(|v| !v.expired())
                        .map(|v| v.data.clone())
                        .collect(),
                )
            })
            .collect()
    }

    /// Nombre de cles stockees.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `true` si vide.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
