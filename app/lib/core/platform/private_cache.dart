// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Cache local de lecture de la zone privée (ADR-0027 §4) —
/// l'action « Lire » exporte une copie en clair vers un dossier
/// temporaire de l'app puis l'ouvre dans le shell natif.
///
/// `null` hors desktop : sur web, `dart:io` n'existe pas et la copie
/// claire n'a pas de destination sûre — l'action est masquée.
library;

import 'private_cache_stub.dart'
    if (dart.library.io) 'private_cache_native.dart' as impl;

/// Dossier temporaire dédié à un export de lecture (`null` = non
/// supporté sur cette plateforme). Le contenu y est en clair —
/// affiché comme « copie non chiffrée » ; le nettoyage relève de la
/// temp du système.
Future<String?> privateReadCacheDir() => impl.privateReadCacheDir();

/// `dir` + `relpath` (`a/b` portable) selon le séparateur natif.
String joinCachePath(String dir, String relpath) =>
    impl.joinCachePath(dir, relpath);

/// `true` sur desktop : « Lire » a une destination temporaire sûre.
const bool supportsPrivateReadCache = impl.supportsPrivateReadCache;
