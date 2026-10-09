// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Web : `mobile_scanner` n'y est pas branché (la caméra navigateur
/// n'est pas utilisée ici) et la page web servie par le daemon agit
/// comme une UI « desktop » — elle affiche le QR.
bool get canScanPairingQr => false;

/// Web servi par le daemon = côté hôte, pas télécommande.
bool get isMobileRemote => false;
