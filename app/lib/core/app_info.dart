// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Metadonnees produit OnionBit — source unique pour la page
/// « A propos » et `showLicensePage`.
library;

import 'package:flutter_riverpod/flutter_riverpod.dart';

const String kAppName = 'OnionBit';

/// Version de l'app — lue du binaire au demarrage via
/// `package_info_plus` (`version:` de `app/pubspec.yaml` est la
/// source unique ; `main.dart` injecte la valeur en override).
/// `'dev'` en secours pour les widgets montes hors bootstrap.
final appVersionProvider = Provider<String>((ref) => 'dev');

const String kAppLicense = 'GPL-3.0-or-later';
const String kAppLicenseUrl = 'https://www.gnu.org/licenses/gpl-3.0.html';

const String kGitHubUrl = 'https://github.com/laurentgeynet-ux/OnionBit';
const String kGitHubIssuesUrl = '$kGitHubUrl/issues';
const String kGitHubDocsUrl = '$kGitHubUrl/tree/master/docs';
const String kGitHubDiscussionsUrl = '$kGitHubUrl/discussions';

const String kAuthor = 'Laurent Geynet';
const int kCopyrightYear = 2026;
