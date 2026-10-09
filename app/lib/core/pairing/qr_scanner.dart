// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Scanner de QR d'appairage — import conditionnel : l'implémentation
/// caméra (`mobile_scanner`) ne s'applique qu'aux cibles natives
/// (gating plateforme ADR-0021 §3 : aucune permission caméra n'est
/// demandée sur web ; le stub y répond `null`).
library;

export 'qr_scanner_stub.dart'
    if (dart.library.io) 'qr_scanner_native.dart';
