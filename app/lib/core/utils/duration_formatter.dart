/// Formattage des durées (ETA de téléchargement, uptime…).
abstract final class DurationFormatter {
  /// `seconds <= 0` renvoie `—` (ETA inconnu/indéfini).
  static String formatSeconds(double seconds) {
    if (seconds <= 0) return '—';
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
