// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Réexport du détecteur de glisser-déposer plateforme — utilisé
/// directement par la zone de dépôt des pièces jointes messagerie
/// (ADR-0019) : contrairement à `DropZone`, aucun filtrage
/// `.torrent`/`.magnet` n'est appliqué ici.
library;

export 'drop_zone_web.dart'
    if (dart.library.io) 'drop_zone_native.dart'
    show DropDetector;
