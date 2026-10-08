// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../../../../core/app_info.dart';
import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/platform/open_url.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../diagnostic/presentation/providers/diagnostic_providers.dart';

/// Page « A propos » — identite du produit (logo, version app +
/// daemon), licence GPL, liens projet et licences tierces.
class AboutPage extends ConsumerWidget {
  const AboutPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final l10n = context.l10n;
    // `version`/`uptime_sec` du daemon (`GET /api/statistics/tribler`
    // — provider partage avec la page Diagnostic).
    final stats = ref.watch(onionbitStatsProvider).value;

    // Bandeau hero pleine largeur + grille de cartes qui remplit
    // l'espace (2 colonnes en large, empilees en etroit).
    return ListView(
      padding: const EdgeInsets.all(AppSpacing.lg),
      children: [
        // Hero : fond teinte `primaryContainer`, logo + badge version.
        Container(
          width: double.infinity,
          padding: const EdgeInsets.symmetric(
            vertical: AppSpacing.xl,
            horizontal: AppSpacing.lg,
          ),
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                theme.colorScheme.primaryContainer.withValues(alpha: 0.55),
                theme.colorScheme.surfaceContainerLowest,
              ],
            ),
            borderRadius: BorderRadius.circular(20),
            border: Border.all(
              color: theme.colorScheme.outlineVariant.withValues(alpha: 0.5),
            ),
          ),
          child: Column(
            children: [
              SvgPicture.asset(
                'assets/branding/logo-horizontal.svg',
                height: 96,
                fit: BoxFit.contain,
                placeholderBuilder: (_) => Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      Icons.shield_outlined,
                      size: 40,
                      color: theme.colorScheme.primary,
                    ),
                    const SizedBox(width: AppSpacing.sm),
                    Text(kAppName, style: theme.textTheme.headlineMedium),
                  ],
                ),
              ),
              const SizedBox(height: AppSpacing.md),
              Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: AppSpacing.md,
                  vertical: AppSpacing.xs,
                ),
                decoration: BoxDecoration(
                  color: theme.colorScheme.primary.withValues(alpha: 0.12),
                  borderRadius: BorderRadius.circular(999),
                ),
                child: Text(
                  'v$kAppVersion — $kAppLicense',
                  style: theme.textTheme.labelMedium?.copyWith(
                    color: theme.colorScheme.primary,
                  ),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: AppSpacing.md),

        LayoutBuilder(
          builder: (context, constraints) {
            final wide = constraints.maxWidth >= 760;
            final versions = _Section(
              icon: Icons.tag_outlined,
              title: l10n.aboutVersions,
              children: [
                _infoRow(theme, l10n.aboutApp, 'v$kAppVersion'),
                _infoRow(
                  theme,
                  l10n.aboutDaemon,
                  stats == null || stats.version.isEmpty
                      ? '—'
                      : 'v${stats.version}',
                ),
                if (stats != null && stats.uptimeSec >= 0)
                  _infoRow(
                    theme,
                    l10n.aboutDaemonUptime,
                    context.fmtDuration(Duration(seconds: stats.uptimeSec)),
                  ),
              ],
            );
            final licenses = _Section(
              icon: Icons.receipt_long_outlined,
              title: l10n.aboutLicenses,
              children: [
                Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: AppSpacing.md,
                    vertical: AppSpacing.sm,
                  ),
                  child: OutlinedButton.icon(
                    icon: const Icon(Icons.receipt_long_outlined),
                    label: Text(l10n.aboutLicenses),
                    onPressed: () => showLicensePage(
                      context: context,
                      applicationName: kAppName,
                      applicationVersion: 'v$kAppVersion',
                      applicationLegalese:
                          '© $kCopyrightYear $kAuthor — $kAppLicense',
                    ),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.md,
                    0,
                    AppSpacing.md,
                    AppSpacing.sm,
                  ),
                  child: Text(
                    l10n.aboutLicensesSub,
                    style: theme.textTheme.bodySmall
                        ?.copyWith(color: theme.colorScheme.outline),
                  ),
                ),
              ],
            );
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (wide)
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(child: versions),
                      const SizedBox(width: AppSpacing.md),
                      Expanded(child: licenses),
                    ],
                  )
                else ...[
                  versions,
                  licenses,
                ],

                _Section(
                  icon: Icons.public_outlined,
                  title: l10n.aboutProject,
                  children: [
                    _linkTile(
                      context,
                      icon: Icons.gavel_outlined,
                      title: kAppLicense,
                      subtitle: '© $kCopyrightYear $kAuthor',
                      url: kAppLicenseUrl,
                    ),
                    _linkTile(
                      context,
                      icon: Icons.code_outlined,
                      title: l10n.aboutSource,
                      subtitle: 'github.com/laurentgeynet-ux/OnionBit',
                      url: kGitHubUrl,
                    ),
                    _linkTile(
                      context,
                      icon: Icons.forum_outlined,
                      title: l10n.aboutCommunity,
                      subtitle: l10n.aboutCommunitySub,
                      url: kGitHubDiscussionsUrl,
                    ),
                    _linkTile(
                      context,
                      icon: Icons.bug_report_outlined,
                      title: l10n.aboutIssues,
                      subtitle: l10n.aboutIssuesSub,
                      url: kGitHubIssuesUrl,
                    ),
                    _linkTile(
                      context,
                      icon: Icons.menu_book_outlined,
                      title: l10n.aboutDocs,
                      subtitle: 'docs/ — roadmap, ADR, CHANGELOG',
                      url: kGitHubDocsUrl,
                    ),
                  ],
                ),
              ],
            );
          },
        ),
      ],
    );
  }

  Widget _infoRow(ThemeData theme, String label, String value) {
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: AppSpacing.md,
        vertical: 2,
      ),
      child: Row(
        children: [
          SizedBox(
            width: 140,
            child: Text(
              label,
              style: theme.textTheme.bodySmall
                  ?.copyWith(color: theme.colorScheme.outline),
            ),
          ),
          Expanded(
            child: SelectableText(
              value,
              style: theme.textTheme.bodyMedium,
            ),
          ),
        ],
      ),
    );
  }

  Widget _linkTile(
    BuildContext context, {
    required IconData icon,
    required String title,
    required String subtitle,
    required String url,
  }) {
    return ListTile(
      dense: true,
      leading: Icon(icon),
      title: Text(title),
      subtitle: Text(subtitle),
      trailing: const Icon(Icons.open_in_new, size: 16),
      onTap: () => openExternalUrl(url),
    );
  }
}

/// Carte de section (icone + titre + contenu) — meme convention
/// visuelle que les sections de la page Reglages.
class _Section extends StatelessWidget {
  const _Section({
    required this.icon,
    required this.title,
    required this.children,
  });

  final IconData icon;
  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: AppSpacing.sm),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: AppSpacing.md,
                vertical: AppSpacing.xs,
              ),
              child: Row(
                children: [
                  Icon(icon, size: 18, color: theme.colorScheme.primary),
                  const SizedBox(width: AppSpacing.sm),
                  Text(title, style: theme.textTheme.titleSmall),
                ],
              ),
            ),
            const Divider(height: 1),
            const SizedBox(height: AppSpacing.sm),
            ...children,
          ],
        ),
      ),
    );
  }
}
