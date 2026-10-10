// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Stub hors-desktop : pas de `dart:io`, pas de cache de lecture.
library;

Future<String?> privateReadCacheDir() async => null;

String joinCachePath(String dir, String relpath) => dir;

const bool supportsPrivateReadCache = false;
