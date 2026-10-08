// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Formattage des octets/débits pour l'affichage (base 1024).
/// Les unités dépendent de la locale : octets en français
/// (`o`, `Ko`…), bytes en anglais (`B`, `KiB`…).
abstract final class ByteFormatter {
  static const List<String> _unitsEn = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  static const List<String> _unitsFr = ['o', 'Ko', 'Mo', 'Go', 'To'];
  static const double _base = 1024;

  /// Formate une quantité d'octets (`locale` = code langue ISO, ex.
  /// `'en'`/`'fr'` — la locale applicative, pas celle du widget).
  static String format(int bytes, [String locale = 'en']) {
    final units = locale == 'fr' ? _unitsFr : _unitsEn;
    if (bytes <= 0) return '0 ${units[0]}';
    var value = bytes.toDouble();
    var unitIndex = 0;
    while (value >= _base && unitIndex < units.length - 1) {
      value /= _base;
      unitIndex++;
    }
    final decimals = unitIndex == 0 ? 0 : 1;
    return '${value.toStringAsFixed(decimals)} ${units[unitIndex]}';
  }

  static String formatRate(int bytesPerSecond, [String locale = 'en']) =>
      '${format(bytesPerSecond, locale)}/s';
}
