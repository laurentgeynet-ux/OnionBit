/// Formattage des octets/débits pour l'affichage (base 1024).
abstract final class ByteFormatter {
  static const List<String> _units = ['o', 'Ko', 'Mo', 'Go', 'To'];
  static const double _base = 1024;

  static String format(int bytes) {
    if (bytes <= 0) return '0 o';
    var value = bytes.toDouble();
    var unitIndex = 0;
    while (value >= _base && unitIndex < _units.length - 1) {
      value /= _base;
      unitIndex++;
    }
    final decimals = unitIndex == 0 ? 0 : 1;
    return '${value.toStringAsFixed(decimals)} ${_units[unitIndex]}';
  }

  static String formatRate(int bytesPerSecond) => '${format(bytesPerSecond)}/s';
}
