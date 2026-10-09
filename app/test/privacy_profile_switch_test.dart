// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:onionbit_ui/core/api/api_client.dart';
import 'package:onionbit_ui/features/privacy/domain/privacy_profile.dart';
import 'package:onionbit_ui/features/privacy/domain/privacy_repository.dart';
import 'package:onionbit_ui/features/privacy/presentation/providers/privacy_providers.dart';
import 'package:onionbit_ui/features/privacy/presentation/widgets/privacy_profile_switch.dart';

import 'helpers/l10n.dart';

/// Sélecteur de profil d'anonymat (ADR-0022 §5) : trois positions
/// reflétant le profil **effectif**, dialogues de conséquences pour
/// `full` (avec saisie de pont si prérequis absent) et retour
/// `legacy`, puce « redémarrage en attente », verrouillage invité.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const stateLegacy = PrivacyProfileState(
    stored: PrivacyProfileKind.legacy,
    effective: PrivacyProfileKind.legacy,
    divergedKeys: [],
    restartPending: false,
    guest: false,
    bridgesConfigured: 0,
  );

  Future<void> pump(
    WidgetTester tester,
    PrivacyProfileState state, {
    _FakePrivacyRepository? repo,
    bool collapsed = false,
    Locale locale = const Locale('en'),
  }) async {
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          privacyProfileProvider.overrideWith((ref) async => state),
          if (repo != null)
            privacyRepositoryProvider.overrideWithValue(repo),
        ],
        child: l10nTestApp(
          Scaffold(body: PrivacyProfileSwitch(collapsed: collapsed)),
          locale: locale,
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('trois positions affichées, effectif mis en avant', (
    tester,
  ) async {
    await pump(tester, stateLegacy);
    expect(find.text('Compatible'), findsOneWidget);
    expect(find.text('Full anonymous'), findsOneWidget);
    expect(find.text('Custom'), findsOneWidget);
    expect(find.byIcon(Icons.lock_outline), findsOneWidget);
    expect(find.byIcon(Icons.enhanced_encryption_outlined), findsOneWidget);
    expect(find.byIcon(Icons.tune), findsOneWidget);
  });

  testWidgets('bascule directe vers custom (aucun dialogue)', (
    tester,
  ) async {
    final repo = _FakePrivacyRepository();
    await pump(tester, stateLegacy, repo: repo);
    await tester.tap(find.text('Custom'));
    await tester.pumpAndSettle();
    expect(repo.calls, ['switch:custom']);
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('full avec pont : dialogue de conséquences puis bascule', (
    tester,
  ) async {
    final repo = _FakePrivacyRepository();
    await pump(
      tester,
      PrivacyProfileState(
        stored: PrivacyProfileKind.legacy,
        effective: PrivacyProfileKind.legacy,
        divergedKeys: const [],
        restartPending: false,
        guest: false,
        bridgesConfigured: 1,
      ),
      repo: repo,
    );
    await tester.tap(find.text('Full anonymous'));
    await tester.pumpAndSettle();
    expect(find.text('Switch to Full anonymous mode?'), findsOneWidget);
    // Pont déjà configuré : pas de champ de saisie.
    expect(find.byType(TextField), findsNothing);
    await tester.tap(find.widgetWithText(FilledButton, 'Enable'));
    await tester.pumpAndSettle();
    expect(repo.calls, ['switch:full']);
  });

  testWidgets('full sans pont : champ lien, refus si invalide puis '
      'ajout + bascule', (tester) async {
    final repo = _FakePrivacyRepository();
    await pump(tester, stateLegacy, repo: repo);
    await tester.tap(find.text('Full anonymous'));
    await tester.pumpAndSettle();
    expect(find.byType(TextField), findsOneWidget);
    // Lien invalide → erreur locale, aucun appel.
    await tester.enterText(find.byType(TextField), 'http://x');
    await tester.tap(
      find.widgetWithText(FilledButton, 'Add bridge and enable'),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('Invalid bridge link'), findsOneWidget);
    expect(repo.calls, isEmpty);
    // Lien valide → POST pont puis PUT profil.
    await tester.enterText(
      find.byType(TextField),
      'onionbit-bridge://127.0.0.1:9999#${'a' * 64}',
    );
    await tester.tap(
      find.widgetWithText(FilledButton, 'Add bridge and enable'),
    );
    await tester.pumpAndSettle();
    expect(repo.calls, [
      'bridge:onionbit-bridge://127.0.0.1:9999#${'a' * 64}',
      'switch:full',
    ]);
  });

  testWidgets('retour de full vers legacy : dialogue allégé', (
    tester,
  ) async {
    final repo = _FakePrivacyRepository();
    await pump(
      tester,
      const PrivacyProfileState(
        stored: PrivacyProfileKind.full,
        effective: PrivacyProfileKind.full,
        divergedKeys: [],
        restartPending: false,
        guest: false,
        bridgesConfigured: 1,
      ),
      repo: repo,
    );
    await tester.tap(find.text('Compatible'));
    await tester.pumpAndSettle();
    expect(find.text('Back to Compatible mode?'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Enable'));
    await tester.pumpAndSettle();
    expect(repo.calls, ['switch:legacy']);
  });

  testWidgets('restart_required : dialogue de redémarrage → shutdown', (
    tester,
  ) async {
    final repo = _FakePrivacyRepository()
      ..switchResult = const PrivacySwitchResult(
        modified: true,
        restartRequired: true,
      );
    await pump(tester, stateLegacy, repo: repo);
    await tester.tap(find.text('Custom'));
    await tester.pumpAndSettle();
    expect(find.text('Restart required'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Restart now'));
    await tester.pumpAndSettle();
    expect(repo.calls, ['switch:custom', 'shutdown']);
    expect(find.text('Restarting daemon…'), findsOneWidget);
  });

  testWidgets('restart_pending : puce visible, rouvre le dialogue', (
    tester,
  ) async {
    final repo = _FakePrivacyRepository();
    await pump(
      tester,
      const PrivacyProfileState(
        stored: PrivacyProfileKind.full,
        effective: PrivacyProfileKind.full,
        divergedKeys: [],
        restartPending: true,
        guest: false,
        bridgesConfigured: 1,
      ),
      repo: repo,
    );
    expect(find.text('Restart pending'), findsOneWidget);
    await tester.tap(find.text('Restart pending'));
    await tester.pumpAndSettle();
    expect(find.text('Restart required'), findsOneWidget);
  });

  testWidgets('session invitée : lignes inertes', (tester) async {
    final repo = _FakePrivacyRepository();
    await pump(
      tester,
      const PrivacyProfileState(
        stored: PrivacyProfileKind.legacy,
        effective: PrivacyProfileKind.legacy,
        divergedKeys: [],
        restartPending: false,
        guest: true,
        bridgesConfigured: 0,
      ),
      repo: repo,
    );
    await tester.tap(find.text('Full anonymous'));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(repo.calls, isEmpty);
  });

  testWidgets('erreur API : snackbar explicite', (tester) async {
    final repo = _FakePrivacyRepository()
      ..switchError = ApiException(409, 'guest_session');
    await pump(tester, stateLegacy, repo: repo);
    await tester.tap(find.text('Custom'));
    await tester.pumpAndSettle();
    expect(
      find.text('Profile switching is unavailable in a guest session'),
      findsOneWidget,
    );
  });

  testWidgets('rail : icône seule + menu des trois positions', (
    tester,
  ) async {
    final repo = _FakePrivacyRepository();
    await pump(tester, stateLegacy, repo: repo, collapsed: true);
    expect(find.text('Compatible'), findsNothing);
    await tester.tap(find.byType(PopupMenuButton<PrivacyProfileKind>));
    await tester.pumpAndSettle();
    expect(find.text('Custom'), findsOneWidget);
    await tester.tap(find.text('Custom'));
    await tester.pumpAndSettle();
    expect(repo.calls, ['switch:custom']);
  });

  testWidgets('FR : libellés localisés', (tester) async {
    await pump(tester, stateLegacy, locale: const Locale('fr'));
    expect(find.text('Full anonyme'), findsOneWidget);
    expect(find.text('Personnalisé'), findsOneWidget);
  });
}

/// Dépôt factice : journalise les appels et permet d'injecter un
/// résultat ou une erreur de bascule.
class _FakePrivacyRepository implements PrivacyRepository {
  final calls = <String>[];
  PrivacySwitchResult? switchResult;
  Object? switchError;

  @override
  Future<PrivacyProfileState> profile() => throw UnimplementedError();

  @override
  Future<PrivacySwitchResult> switchProfile(
    PrivacyProfileKind target,
  ) async {
    calls.add('switch:${target.wire}');
    if (switchError != null) throw switchError!;
    return switchResult ??
        const PrivacySwitchResult(modified: true, restartRequired: false);
  }

  @override
  Future<void> addBridge(String link) async => calls.add('bridge:$link');

  @override
  Future<void> shutdown() async => calls.add('shutdown');
}
