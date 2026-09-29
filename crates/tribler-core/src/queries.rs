//! Port de `tribler/core/database/queries.py` : conversion du texte
//! utilisateur en requete FTS (`to_fts_query`) et extraction des
//! termes (`fts_terms`, pour notre moteur `LIKE` AND — remplacant
//! documente de FTS5).

/// Terme FTS : sequence `\w+` (regex `\w+` Python = alphanumerique
/// Unicode + `_`).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `to_fts_query` Python : `"mot1" "mot2"` (mots `\w+` entre
/// guillemets, joints par espace). `None` si aucun mot —
/// l'appelant omet alors `txt_filter` (Python envoie `null` qu'on
/// supprime du JSON, equivalent au `not words -> None` Pony).
pub fn to_fts_query(text: &str) -> Option<String> {
    let terms = fts_terms(text);
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Extrait les termes `\w+` d'un texte — y compris dans une requete
/// deja formatee FTS (`"a" "b"` -> `a`, `b`), pour la correspondance
/// `LIKE` AND de notre base.
pub fn fts_terms(text: &str) -> Vec<String> {
    text.split(|c: char| !is_word_char(c))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fts_quote_chaque_mot() {
        assert_eq!(
            to_fts_query("ubuntu 24.04, lts!").as_deref(),
            Some("\"ubuntu\" \"24\" \"04\" \"lts\"")
        );
        assert_eq!(to_fts_query(""), None);
        assert_eq!(to_fts_query("   ,,, "), None);
    }

    #[test]
    fn fts_terms_accepte_les_quotes() {
        assert_eq!(fts_terms("\"a\" \"b\""), vec!["a", "b"]);
        assert_eq!(fts_terms("x_y z"), vec!["x_y", "z"]);
    }
}
