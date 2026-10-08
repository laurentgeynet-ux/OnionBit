// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/widgets.dart';

import '../../l10n/app_localizations.dart';
import '../utils/byte_formatter.dart';
import '../utils/duration_formatter.dart';

/// Raccourci d'accès aux chaînes localisées : `context.l10n.xxx`.
extension L10nX on BuildContext {
  AppLocalizations get l10n => AppLocalizations.of(this);
}

/// Formatteurs « sensibles à la langue » — unités `o`/`Ko`/`Mo`
/// (FR) vs `B`/`KiB`/`MiB` (EN), `j` vs `d` pour les jours.
extension FmtL10nX on BuildContext {
  /// Code langue de la locale applicative active (`en`, `fr`…).
  String get uiLang => Localizations.localeOf(this).languageCode;

  /// Quantité d'octets formatée dans les unités de la locale.
  String fmtBytes(int bytes) => ByteFormatter.format(bytes, uiLang);

  /// Débit formaté (`…/s`) dans les unités de la locale.
  String fmtRate(int bytesPerSecond) =>
      ByteFormatter.formatRate(bytesPerSecond, uiLang);

  /// Durée/ETA formatée dans la locale (`j` vs `d`).
  String fmtDuration(Duration duration) =>
      DurationFormatter.format(duration, uiLang);

  /// ETA en secondes formatée dans la locale (`—` si indéfinie).
  String fmtEta(double seconds) =>
      DurationFormatter.formatSeconds(seconds, uiLang);
}
