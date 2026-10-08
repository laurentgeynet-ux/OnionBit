// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'app_notification.dart';

/// Centre de notifications de l'app — liste bornée (100 dernières),
/// compteur de non-lues. Alimenté par `NotificationsListener`
/// (événements SSE) et consulté par la cloche de la `TopBar`.
final notificationsProvider =
    NotifierProvider<NotificationsNotifier, List<AppNotification>>(
      NotificationsNotifier.new,
    );

/// Nombre de notifications non lues (badge de la cloche).
final unreadNotificationsProvider = Provider<int>(
  (ref) => ref.watch(notificationsProvider).where((n) => !n.read).length,
);

class NotificationsNotifier extends Notifier<List<AppNotification>> {
  static const _max = 100;

  @override
  List<AppNotification> build() => const [];

  /// Ajoute en tête (la plus récente d'abord).
  void push(AppNotification n) {
    state = [n, ...state].take(_max).toList();
  }

  void markAllRead() {
    for (final n in state) {
      n.read = true;
    }
    state = [...state];
  }

  void clear() => state = const [];
}
