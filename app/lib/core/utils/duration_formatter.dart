// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Formattage des durées (ETA de téléchargement, uptime…).
abstract final class DurationFormatter {
  /// `seconds <= 0` renvoie `—` (ETA inconnu/indéfini). Au-delà de
  /// ~300 ans, l'ETA est de fait infinie (débit nul côté backend =
  /// `total/1e-6`) et `Duration(seconds:)` déborde en int64 —
  /// microsecondes signées — affichant des nombres négatifs absurdes.
  static String formatSeconds(double seconds) {
    if (!seconds.isFinite || seconds <= 0 || seconds > 1e10) return '—';
    return format(Duration(seconds: seconds.round()));
  }

  static String format(Duration duration) {
    if (duration.inSeconds < 60) return '${duration.inSeconds} s';
    if (duration.inMinutes < 60) {
      return '${duration.inMinutes} min ${duration.inSeconds % 60} s';
    }
    if (duration.inHours < 24) {
      return '${duration.inHours} h ${duration.inMinutes % 60} min';
    }
    return '${duration.inDays} j ${duration.inHours % 24} h';
  }
}
