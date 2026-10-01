# ADR-0009 — Internationalisation de l'app Flutter via gen_l10n

Statut : Acceptée (2026-10-01).

## Contexte

L'app `onionbit_ui` est née avec des chaînes françaises en dur dans ~30
fichiers Dart (~300 littéraux). L'utilisateur a demandé une
traduction complète en anglais, l'anglais affiché **par défaut**, tout
en conservant le français. Il fallait un mécanisme standard,
extensible (autres langues futures) et compatible avec la règle
ADR-0005 (« code et docs en français »).

## Décision

- **Pipeline standard Flutter** : `flutter_localizations` + `intl` +
  `gen_l10n` piloté par `app/l10n.yaml`. Les sources de traduction
  vivent dans `app/lib/l10n/` : `app_en.arb` = gabarit/source de
  vérité, `app_fr.arb` = traduction française complète.
- **Fichiers générés non commités** : `app_localizations*.dart` sont
  produits par `flutter gen-l10n` (ou implicitement au build) et
  ignorés par `app/.gitignore`.
- **Locale applicative** : enum `AppLocale { system, en, fr }`,
  provider `localeSettingsProvider` (AsyncNotifier) persisté dans
  `SharedPreferences` sous la clé `ui.locale`. **Défaut : `en`**
  (choix produit), jamais la locale OS sauf sélection explicite
  « System ». Bascule à chaud via `MaterialApp.router(locale:)`.
- **Accès uniforme** : `context.l10n.*` (extension `L10nX` dans
  `core/l10n/l10n_ext.dart`). Paramètres et pluriels via ICU dans les
  ARB (`{count, plural, …}`, `{message}`), pas de concaténation
  manuelle de préfixes traduits.
- **Frontière domaine/présentation** : les enums et modèles ne
  portent plus de libellé (ex. `DownloadFilter` n'a que `queryKey` ;
  `DownloadFilterX.label(l10n)` mappe vers l'ARB).
- **Unités et durées** : `ByteFormatter`/`DurationFormatter` prennent
  le code langue (`o`/`Ko`/`Mo` en FR, `B`/`KiB`/`MiB` en EN ; `j`/`d`
  pour les jours) — raccourcis `context.fmtBytes`/`fmtRate`/`fmtEta`.
- **Hors périmètre UI** (restent en français, ADR-0005) :
  commentaires, docstrings, journaux développeur (`uiLog`,
  `debugPrint`), ADR/CHANGELOG/roadmap. Les noms de langue restent des
  autonymes (`English`, `Français`) et les listes `keywords:` de
  recherche des réglages gardent des termes bilingues.
- **Garde-fou** : `scripts/check_i18n.ps1` échoue si un littéral à
  consonance française réapparaît dans `app/lib/` (commentaires,
  journaux, autonyme et keywords exclus par allowlist).

## Conséquences

- Nouvelle installation ⇒ UI en anglais ; français sélectionnable dans
  Réglages → Apparence ; bascule effective sans redémarrage.
- Toute nouvelle chaîne UI = une clé dans `app_en.arb` + sa traduction
  dans `app_fr.arb`, puis `context.l10n.*`.
- Ajout d'une langue = un `app_<code>.arb` + une entrée `AppLocale`.
- Les tests de widgets passent par le helper `pumpApp(locale: ...)`
  (delegates de localisation inclus) et les assertions visent les
  chaînes anglaises (locale par défaut).
