import 'package:flutter/material.dart';

/// Sévérité d'une notification du centre.
enum AppNotificationSeverity { info, success, warning, error }

/// Une entrée du centre de notifications (session uniquement —
/// pas de persistance disque).
class AppNotification {
  AppNotification({
    required this.title,
    required this.message,
    this.severity = AppNotificationSeverity.info,
    DateTime? timestamp,
  }) : timestamp = timestamp ?? DateTime.now();

  final String title;
  final String message;
  final AppNotificationSeverity severity;
  final DateTime timestamp;
  bool read = false;

  IconData get icon => switch (severity) {
    AppNotificationSeverity.info => Icons.info_outline,
    AppNotificationSeverity.success => Icons.check_circle_outline,
    AppNotificationSeverity.warning => Icons.warning_amber,
    AppNotificationSeverity.error => Icons.error_outline,
  };
}
