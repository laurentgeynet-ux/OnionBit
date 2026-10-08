// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/api/api_client.dart';
import 'package:onionbit_ui/core/identity/identity_gate.dart';
import 'package:onionbit_ui/features/settings/domain/settings_repository.dart';
import 'package:onionbit_ui/features/settings/presentation/providers/settings_providers.dart';

import 'helpers/l10n.dart';

/// Dépôt minimal pour les écrans du gate (ADR-0016, 48e) — les
/// appels identité sont instrumentés, le reste renvoie des vides.
class _FakeRepo implements SettingsRepository {
  String? unlockedWith;
  bool guestCalled = false;
  bool createCalled = false;
  Object? unlockError;

  @override
  Future<Map<String, dynamic>> get() async => {};
  @override
  Future<void> update(Map<String, dynamic> settings) async {}
  @override
  Future<void> shutdown() async {}
  @override
  Future<Map<String, int>> dirSpace({String? directory}) async =>
      {'total': 0, 'used': 0, 'free': 0};
  @override
  Future<List<Map<String, dynamic>>> rssItems() async => [];
  @override
  Future<void> setRssFeeds(List<String> urls) async {}
  @override
  Future<Map<String, dynamic>> versions() async => {};
  @override
  Future<Map<String, dynamic>> checkVersion() async => {};
  @override
  Future<String?> identityPublicKey() async => null;
  @override
  Future<Map<String, dynamic>> identityStatus() async => {};
  @override
  Future<String?> identityRecoveryPhrase({String? lang}) async => null;
  @override
  Future<Map<String, dynamic>> identityExport({String? password}) async => {};
  @override
  Future<void> identityRestore(
    String keyHex, {
    String? password,
    bool forceLegacy = false,
  }) async {}
  @override
  Future<void> identityRestorePhrase(String phrase) async {}
  @override
  Future<void> identitySetAtRest({
    required bool enabled,
    required String password,
  }) async {}
  @override
  Future<void> identityCreate({String? password}) async =>
      createCalled = true;
  @override
  Future<void> identityGuest() async => guestCalled = true;
  @override
  Future<void> identityUnlock(String password) async {
    unlockedWith = password;
    final e = unlockError;
    if (e != null) throw e;
  }
}

Widget _gateApp(_FakeRepo repo, Widget home) => ProviderScope(
  overrides: [settingsRepositoryProvider.overrideWithValue(repo)],
  child: l10nTestApp(home),
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('gate pending : trois résolutions proposées', (tester) async {
    final repo = _FakeRepo();
    await tester.pumpWidget(
      _gateApp(repo, const IdentityGatePage(locked: false)),
    );
    await tester.pumpAndSettle();

    // Les trois cartes + le libellé d'ouverture du formulaire restore.
    expect(find.text('New identity'), findsOneWidget);
    expect(find.text('Restore an identity'), findsOneWidget);
    expect(find.text('Guest session'), findsOneWidget);
  });

  testWidgets('gate pending : « nouvelle identité » appelle create', (
    tester,
  ) async {
    final repo = _FakeRepo();
    await tester.pumpWidget(
      _gateApp(repo, const IdentityGatePage(locked: false)),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('New identity'));
    await tester.pumpAndSettle();
    expect(repo.createCalled, isTrue);
    expect(repo.guestCalled, isFalse);
  });

  testWidgets('gate pending : invité appelle guest', (tester) async {
    final repo = _FakeRepo();
    await tester.pumpWidget(
      _gateApp(repo, const IdentityGatePage(locked: false)),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Guest session'));
    await tester.pumpAndSettle();
    expect(repo.guestCalled, isTrue);
  });

  testWidgets('gate locked : unlock envoie le mot de passe', (
    tester,
  ) async {
    final repo = _FakeRepo();
    await tester.pumpWidget(
      _gateApp(repo, const IdentityGatePage(locked: true)),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, 's3cret');
    await tester.tap(find.text('Unlock'));
    await tester.pumpAndSettle();
    expect(repo.unlockedWith, 's3cret');
  });

  testWidgets('gate locked : 400 → « mot de passe incorrect », '
      '429 → message de rate-limit', (tester) async {
    final repo = _FakeRepo()..unlockError = ApiException(400, 'x');
    await tester.pumpWidget(
      _gateApp(repo, const IdentityGatePage(locked: true)),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, 'faux');
    await tester.tap(find.text('Unlock'));
    await tester.pumpAndSettle();
    expect(find.text('Incorrect password'), findsOneWidget);

    repo.unlockError = ApiException(429, 'x');
    await tester.enterText(find.byType(TextField).first, 'faux2');
    await tester.tap(find.text('Unlock'));
    await tester.pumpAndSettle();
    expect(
      find.text('Too many attempts — wait a moment and retry'),
      findsOneWidget,
    );
  });

  testWidgets('gate locked : porte de sortie invitée', (tester) async {
    final repo = _FakeRepo();
    await tester.pumpWidget(
      _gateApp(repo, const IdentityGatePage(locked: true)),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Continue as guest instead'));
    await tester.pumpAndSettle();
    expect(repo.guestCalled, isTrue);
  });

  testWidgets('bandeau invité visible', (tester) async {
    await tester.pumpWidget(
      l10nTestApp(const Scaffold(body: GuestBanner())),
    );
    expect(find.byIcon(Icons.person_off_outlined), findsOneWidget);
  });
}
