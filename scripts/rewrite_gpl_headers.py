# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later
"""ADR-0020 §7 — réécriture des notices GPL historiques.

Remplace le descripteur historique
    « This file is part of OnionBit - a Rust port of the Tribler daemon. »
par « This file is part of OnionBit. » dans toutes les syntaxes de
commentaires (//, #, <!-- -->, /* * */), sur tout le dépôt hors exclusions.

Balayage par contenu, pas par position : la notice peut démarrer en
ligne 2 après un shebang (`.sh`, `.py`) ou après un BOM UTF-8 (`.ps1`).
Seule la ligne de descripteur est modifiée — les blocs restent valides
et les citations en prose (markdown, docs) ne sont pas touchées.

Usage : python scripts/rewrite_gpl_headers.py [--dry-run]
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Premiers composants exclus ; '.dart_tool' et 'node_modules' exclus où
# qu'ils apparaissent ; 'build' exclu uniquement sous app/ (artefact Flutter).
EXCLUDE_TOP = {".git", "vendor", "target", ".idea", ".vs"}
EXCLUDE_ANY = {".dart_tool", "node_modules", "__pycache__"}

NEW_DESC = "This file is part of OnionBit."

# Ligne de commentaire portant l'ancien descripteur, ancrée en début de
# ligne : uniquement espaces/tabulations avant le marqueur, puis seulement
# des blancs entre marqueur et phrase — les citations en prose (markdown,
# guillemets, code) ne peuvent pas correspondre. Le bloc /* * */ est couvert
# via son ouverture /* (le descripteur est toujours en tête de notice).
OLD_LINE = re.compile(
    r"(?m)^(?P<lead>[ \t]*(?://|#|<!--|/\*)[ \t]*)"
    r"This file is part of OnionBit[ \t]*-[ \t]*a Rust port of the Tribler daemon\."
)
# Forme nouvelle sans point final (fin de ligne immédiate) — normalisée.
BARE_NEW_LINE = re.compile(
    r"(?m)^(?P<lead>[ \t]*(?://|#|<!--|/\*)[ \t]*)"
    r"This file is part of OnionBit(?=[ \t]*\r?$)"
)


def excluded(rel: Path) -> bool:
    parts = rel.parts
    if parts and parts[0] in EXCLUDE_TOP:
        return True
    if any(p in EXCLUDE_ANY for p in parts):
        return True
    return len(parts) >= 2 and parts[0] == "app" and parts[1] == "build"


def rewrite_file(path: Path, dry_run: bool) -> int:
    """Retourne le nombre de lignes réécrites (0 si rien à faire)."""
    raw = path.read_bytes()
    bom = raw.startswith(b"\xef\xbb\xbf")
    try:
        text = raw.decode("utf-8-sig")
    except UnicodeDecodeError:
        return 0
    text, n_old = OLD_LINE.subn(
        lambda m: m.group("lead") + NEW_DESC, text
    )
    text, n_bare = BARE_NEW_LINE.subn(
        lambda m: m.group("lead") + NEW_DESC, text
    )
    changed = n_old + n_bare
    if changed and not dry_run:
        out = text.encode("utf-8")
        if bom:
            out = b"\xef\xbb\xbf" + out
        path.write_bytes(out)
    return changed


def main() -> int:
    dry_run = "--dry-run" in sys.argv
    files_changed = 0
    lines_changed = 0
    for path in sorted(ROOT.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(ROOT)
        if excluded(rel):
            continue
        n = rewrite_file(path, dry_run)
        if n:
            files_changed += 1
            lines_changed += n
            print(f"  {rel} ({n})")
    mode = "[dry-run] " if dry_run else ""
    print(f"{mode}{files_changed} fichier(s), {lines_changed} ligne(s) réécrite(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
