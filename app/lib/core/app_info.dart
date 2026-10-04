// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

/// Metadonnees produit OnionBit — source unique pour la page
/// « A propos » et `showLicensePage`.
library;

const String kAppName = 'OnionBit';

/// Version de l'app — a synchroniser avec `version:` de
/// `app/pubspec.yaml` (le suffixe `+N` est le build number).
const String kAppVersion = '0.8.0';

const String kAppLicense = 'GPL-3.0-or-later';
const String kAppLicenseUrl = 'https://www.gnu.org/licenses/gpl-3.0.html';

const String kGitHubUrl = 'https://github.com/laurentgeynet-ux/OnionBit';
const String kGitHubIssuesUrl = '$kGitHubUrl/issues';
const String kGitHubDocsUrl = '$kGitHubUrl/tree/master/docs';
const String kGitHubDiscussionsUrl = '$kGitHubUrl/discussions';

const String kAuthor = 'Laurent Geynet';
const int kCopyrightYear = 2026;
