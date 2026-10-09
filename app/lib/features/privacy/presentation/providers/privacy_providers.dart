// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/api/events.dart';
import '../../../../core/api/sse_client.dart';
import '../../../../core/di/providers.dart';
import '../../data/rest_privacy_repository.dart';
import '../../domain/privacy_profile.dart';
import '../../domain/privacy_repository.dart';

/// Dépôt profil d'anonymat (ADR-0022).
final privacyRepositoryProvider = Provider<PrivacyRepository>(
  (ref) => RestPrivacyRepository(ref.watch(apiClientProvider)),
);

/// Posture d'anonymat (`GET /api/privacy/profile`) : profil stocké,
/// profil effectif dérivé, clés divergentes, `restart_pending`,
/// session invitée et ponts configurés.
///
/// SSE `settings_changed` → rechargement : une bascule faite par un
/// autre client, ou une divergence née d'une édition manuelle des
/// réglages, bascule l'affichage du sélecteur sur « Personnalisé »
/// sans intervention locale.
final privacyProfileProvider =
    FutureProvider.autoDispose<PrivacyProfileState>((ref) {
      ref.listen<AsyncValue<SseEvent>>(daemonEventsProvider, (_, next) {
        if (next.value?.topic == EventTopics.settingsChanged) {
          ref.invalidateSelf();
        }
      });
      return ref.watch(privacyRepositoryProvider).profile();
    });
