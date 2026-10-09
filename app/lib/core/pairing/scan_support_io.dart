// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:io';

/// `mobile_scanner` couvre Android/iOS/macOS (déclaré dans son
/// pubspec — Windows/Linux ne l'embarquent pas : le bouton de scan
/// est masqué sur ces cibles).
bool get canScanPairingQr =>
    Platform.isAndroid || Platform.isIOS || Platform.isMacOS;

/// Le téléphone est la télécommande du daemon desktop (modèle du
/// projet) : il scanne le QR de l'hôte, il n'en affiche pas.
bool get isMobileRemote => Platform.isAndroid || Platform.isIOS;
