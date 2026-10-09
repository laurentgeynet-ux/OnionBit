// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/api/api_client.dart';
import 'package:onionbit_ui/core/identity/identity_gate.dart';
import 'package:onionbit_ui/features/settings/domain/settings_repository.dart';
import 'package:onionbit_ui/features/settings/presentation/providers/settings_providers.dart';

import 'package:onionbit_ui/core/identity/onboarding_wizard.dart';
import 'package:onionbit_ui/core/l10n/locale_settings.dart';
import 'package:onionbit_ui/l10n/app_localizations.dart';
import 'package:shared_preferences/shared_preferences.dart';

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
  Future<Map<String, int>> dirSpace({String? directory, String? area}) async =>
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
  Future<String?> identityRecoveryPhrase({String? lang}) async =>
      'abandon ability able about above absent absorb abstract';
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
  @override
  Future<Map<String, dynamic>> privateZone() async =>
      {'state': 'locked', 'downloads': const [], 'orphans': const {}};
  @override
  Future<void> purgePrivateOrphans() async {}
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

  testWidgets('gate pending : « nouvelle identité » → étape création '
      'puis create (étape 76 — wizard)', (tester) async {
    final repo = _FakeRepo();
    await tester.pumpWidget(
      _gateApp(repo, const IdentityGatePage(locked: false)),
    );
    await tester.pumpAndSettle();
    // Étape « choix » : la carte n'appelle pas encore create, elle
    // ouvre l'étape dédiée (mot de passe at-rest optionnel).
    await tester.tap(find.text('New identity'));
    await tester.pumpAndSettle();
    expect(repo.createCalled, isFalse);
    await tester.tap(find.text('Create my identity'));
    await tester.pumpAndSettle();
    expect(repo.createCalled, isTrue);
    expect(repo.guestCalled, isFalse);
  });

  testWidgets('gate pending : phrase de récupération remise à '
      'l\'overlay global après create (aucun Navigator sous le gate)',
      (tester) async {
    final repo = _FakeRepo();
    final container = ProviderContainer(
      overrides: [settingsRepositoryProvider.overrideWithValue(repo)],
    );
    addTearDown(container.dispose);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: l10nTestApp(const IdentityGatePage(locked: false)),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('New identity'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Create my identity'));
    await tester.pumpAndSettle();
    // Le fake rend une phrase → elle est publiée au provider : c'est
    // `app.dart` qui la rend en overlay (pendant `pending` le gate
    // remplace le Navigator — un `showDialog` planterait en silence).
    expect(
      container.read(pendingRecoveryPhraseProvider),
      'abandon ability able about above absent absorb abstract',
    );
  });

  testWidgets('phrase backup : confirmation exigée avant « Done », '
      'onDone referme l\'overlay', (tester) async {
    var done = false;
    await tester.pumpWidget(
      l10nTestApp(
        Scaffold(
          body: PhraseBackupDialog(
            phrase:
                'abandon ability able about above absent absorb abstract',
            onDone: () => done = true,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Recovery phrase'), findsOneWidget);
    // « Done » reste désactivé tant que la phrase n'est pas confirmée.
    final button = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Done'),
    );
    expect(button.onPressed, isNull);
    await tester.tap(find.byType(CheckboxListTile));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Done'));
    expect(done, isTrue);
  });

  testWidgets('gate pending : le sélecteur de langue relocalise le '
      'wizard et persiste le choix', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final repo = _FakeRepo();
    // Harnais fidèle à la prod : `MaterialApp.locale` suit
    // `localeSettingsProvider` (l10nTestApp figerait `en`).
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(repo),
        ],
        child: Consumer(
          builder: (context, ref, _) {
            final locale = resolveFlutterLocale(
              ref.watch(localeSettingsProvider).value,
            );
            return MaterialApp(
              locale: locale,
              localizationsDelegates:
                  AppLocalizations.localizationsDelegates,
              supportedLocales: AppLocalizations.supportedLocales,
              home: const IdentityGatePage(locked: false),
            );
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Choose your identity'), findsOneWidget);

    await tester.tap(find.text('Français'));
    await tester.pumpAndSettle();
    expect(find.text('Choisissez votre identité'), findsOneWidget);
    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString('ui.locale'), 'fr');
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
