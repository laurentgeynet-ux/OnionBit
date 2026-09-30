import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../features/downloads/domain/download_filter.dart';
import '../../features/downloads/presentation/providers/downloads_providers.dart';
import '../../features/downloads/presentation/widgets/add_download_dialog.dart';
import '../router/nav_catalog.dart';
import '../theme/app_theme.dart';
import 'nav_destination.dart';

/// Sidebar fixe type Tribler (~216 px) : bouton « Ajouter » en tête,
/// groupe « Bibliothèque » (Téléchargements + sous-filtres avec
/// compteurs, Rechercher) puis groupe « Système » (Réglages,
/// Diagnostic). Items en pilule arrondie, badge d'erreurs sur
/// l'entrée Téléchargements.
class AppSidebar extends ConsumerWidget {
  const AppSidebar({
    super.key,
    required this.currentPath,
    required this.currentQuery,
  });

  /// `state.matchedLocation` courant.
  final String currentPath;

  /// `state.uri.queryParameters` courant (sous-filtre `f`).
  final Map<String, String> currentQuery;

  static const double width = 216;

  static const _filterIcons = {
    DownloadFilter.downloading: Icons.downloading,
    DownloadFilter.completed: Icons.check_circle_outline,
    DownloadFilter.active: Icons.bolt,
    DownloadFilter.inactive: Icons.pause_circle_outline,
  };

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final scheme = Theme.of(context).colorScheme;
    final downloads = ref.watch(downloadsProvider).value;
    final errors = downloads?.where((d) => d.isError).length ?? 0;
    return Material(
      color: scheme.surface,
      child: SizedBox(
        width: width,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.all(AppSpacing.md),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      Icon(Icons.shield_outlined, color: scheme.primary),
                      const SizedBox(width: AppSpacing.sm),
                      Flexible(
                        child: Text(
                          'Tribler-Rust',
                          style: Theme.of(context).textTheme.titleSmall,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: AppSpacing.md),
                  FilledButton.icon(
                    onPressed: () => AddDownloadDialog.show(context),
                    icon: const Icon(Icons.add),
                    label: const Text('Ajouter'),
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.symmetric(horizontal: AppSpacing.sm),
                children: [
                  const _GroupLabel('Bibliothèque'),
                  _NavItem(
                    d: kNavCatalog[0],
                    selected:
                        currentPath == '/downloads' &&
                        currentQuery['f'] == null,
                    badge: errors > 0 ? _ErrorBadge(count: errors) : null,
                  ),
                  // Sous-filtres Téléchargements (sauf « Tous » =
                  // entrée parente).
                  for (final f in DownloadFilter.values.skip(1))
                    _FilterItem(
                      filter: f,
                      icon: _filterIcons[f],
                      selected:
                          currentPath == '/downloads' &&
                          currentQuery['f'] == f.queryKey,
                      count: downloads?.where(f.matches).length,
                    ),
                  _NavItem(
                    d: kNavCatalog[1],
                    selected: currentPath == '/search',
                  ),
                  const _GroupLabel('Système'),
                  _NavItem(
                    d: kNavCatalog[3],
                    selected: currentPath == '/settings',
                  ),
                  _NavItem(
                    d: kNavCatalog[2],
                    selected: currentPath == '/diagnostic',
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// En-tête de groupe de la sidebar (« Bibliothèque », « Système »).
class _GroupLabel extends StatelessWidget {
  const _GroupLabel(this.label);

  final String label;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        AppSpacing.sm,
        AppSpacing.sm,
        AppSpacing.sm,
        AppSpacing.xs,
      ),
      child: Text(
        label.toUpperCase(),
        style: theme.textTheme.labelSmall?.copyWith(
          color: theme.colorScheme.outline,
          letterSpacing: 0.8,
        ),
      ),
    );
  }
}

/// Item de navigation principal en pilule (surbrillance arrondie).
class _NavItem extends StatelessWidget {
  const _NavItem({required this.d, required this.selected, this.badge});

  final NavDestinationSpec d;
  final bool selected;

  /// Badge en bout d'item (ex. compteur d'erreurs).
  final Widget? badge;

  @override
  Widget build(BuildContext context) {
    return _SidebarItem(
      icon: selected ? d.selectedIcon : d.icon,
      label: d.label,
      selected: selected,
      trailing: badge,
      onTap: () => context.go(d.path),
    );
  }
}

/// Item de sous-filtre indenté avec icône et compteur badge.
class _FilterItem extends StatelessWidget {
  const _FilterItem({
    required this.filter,
    required this.selected,
    this.icon,
    this.count,
  });

  final DownloadFilter filter;
  final bool selected;
  final IconData? icon;
  final int? count;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(left: AppSpacing.lg),
      child: _SidebarItem(
        icon: icon,
        label: filter.label,
        selected: selected,
        trailing: count == null ? null : _CountBadge(count: count!),
        onTap: () => context.go('/downloads?f=${filter.queryKey}'),
      ),
    );
  }
}

/// Brique commune : pilule `InkWell` arrondie, icône + label +
/// trailing optionnel. Style M3 « navigation rail » adapté à la
/// sidebar fixe.
class _SidebarItem extends StatelessWidget {
  const _SidebarItem({
    required this.label,
    required this.selected,
    required this.onTap,
    this.icon,
    this.trailing,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;
  final IconData? icon;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final color = selected ? scheme.onPrimaryContainer : scheme.onSurface;
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: InkWell(
        borderRadius: BorderRadius.circular(24),
        onTap: onTap,
        child: Container(
          decoration: BoxDecoration(
            color: selected ? scheme.primaryContainer : Colors.transparent,
            borderRadius: BorderRadius.circular(24),
          ),
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.sm,
            vertical: AppSpacing.sm - 2,
          ),
          child: Row(
            children: [
              if (icon != null) ...[
                Icon(icon, size: 18, color: color),
                const SizedBox(width: AppSpacing.sm),
              ],
              Expanded(
                child: Text(
                  label,
                  style: theme.textTheme.bodyMedium?.copyWith(
                    color: color,
                    fontWeight: selected ? FontWeight.w600 : FontWeight.normal,
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              ?trailing,
            ],
          ),
        ),
      ),
    );
  }
}

/// Badge de compteur neutre (fond `surfaceContainerHighest`).
class _CountBadge extends StatelessWidget {
  const _CountBadge({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Text('$count', style: Theme.of(context).textTheme.labelSmall),
    );
  }
}

/// Badge d'erreurs (fond `errorContainer`) — attirer l'œil sur les
/// téléchargements en échec sans changer de page.
class _ErrorBadge extends StatelessWidget {
  const _ErrorBadge({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
      decoration: BoxDecoration(
        color: scheme.errorContainer,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Text(
        '$count',
        style: Theme.of(context).textTheme.labelSmall
            ?.copyWith(color: scheme.onErrorContainer),
      ),
    );
  }
}
