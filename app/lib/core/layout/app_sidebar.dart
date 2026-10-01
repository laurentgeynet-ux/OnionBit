import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../features/downloads/domain/download_filter.dart';
import '../../features/downloads/presentation/providers/downloads_providers.dart';
import '../../features/downloads/presentation/widgets/add_download_dialog.dart';
import '../../features/settings/presentation/providers/settings_providers.dart';
import '../di/providers.dart';
import '../router/nav_catalog.dart';
import '../theme/app_theme.dart';
import '../utils/byte_formatter.dart';
import '../config/ui_prefs.dart';
import 'nav_destination.dart';

/// Replie/déplie le groupe des sous-filtres Téléchargements (état de
/// session uniquement).
final sidebarFiltersExpandedProvider =
    NotifierProvider<_FiltersExpandedNotifier, bool>(
      _FiltersExpandedNotifier.new,
    );

class _FiltersExpandedNotifier extends Notifier<bool> {
  @override
  bool build() => true;

  void init(bool v) => state = v;

  void toggle() {
    state = !state;
    unawaited(uiPrefsWrite('ui.filtersExpanded', state));
  }
}

/// Bascule sidebar pleine largeur ↔ rail icônes seules (état de
/// session uniquement).
final sidebarCollapsedProvider = NotifierProvider<_CollapsedNotifier, bool>(
  _CollapsedNotifier.new,
);

class _CollapsedNotifier extends Notifier<bool> {
  @override
  bool build() => false;

  void init(bool v) => state = v;

  void toggle() {
    state = !state;
    unawaited(uiPrefsWrite('ui.sidebarCollapsed', state));
  }
}

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

  /// Largeur du rail rétracté (icônes seules).
  static const double collapsedWidth = 72;

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
    final filtersExpanded = ref.watch(sidebarFiltersExpandedProvider);
    final collapsed = ref.watch(sidebarCollapsedProvider);
    return Material(
      color: scheme.surface,
      child: SizedBox(
        width: collapsed ? collapsedWidth : width,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: EdgeInsets.all(
                collapsed ? AppSpacing.xs : AppSpacing.md,
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  // Logo OnionBit (SVG brandé) — fallback icône si
                  // l'asset n'est pas embarqué dans ce build.
                  SvgPicture.asset(
                    collapsed
                        ? 'assets/branding/icon.svg'
                        : 'assets/branding/logo-horizontal.svg',
                    height: collapsed ? 36 : 44,
                    fit: BoxFit.contain,
                    alignment: Alignment.center,
                    placeholderBuilder: (_) => Row(
                      children: [
                        Icon(Icons.shield_outlined, color: scheme.primary),
                        const SizedBox(width: AppSpacing.sm),
                        Flexible(
                          child: Text(
                            'OnionBit',
                            style: Theme.of(context).textTheme.titleSmall,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: AppSpacing.md),
                  if (collapsed)
                    Tooltip(
                      message: 'Ajouter un téléchargement',
                      child: FilledButton(
                        style: FilledButton.styleFrom(
                          padding: EdgeInsets.zero,
                          minimumSize: const Size(48, 40),
                        ),
                        onPressed: () => AddDownloadDialog.show(context),
                        child: const Icon(Icons.add),
                      ),
                    )
                  else
                    FilledButton.icon(
                      onPressed: () => AddDownloadDialog.show(context),
                      icon: const Icon(Icons.add),
                      label: const Text('Ajouter'),
                    ),
                  if (!collapsed) ...[
                    const SizedBox(height: AppSpacing.sm),
                    const _SpeedsRow(),
                  ],
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.symmetric(horizontal: AppSpacing.sm),
                children: [
                  if (!collapsed) const _GroupLabel('Bibliothèque'),
                  _NavItem(
                    d: kNavCatalog[0],
                    collapsed: collapsed,
                    selected:
                        currentPath == '/downloads' &&
                        currentQuery['f'] == null,
                    badge: errors > 0 ? _ErrorBadge(count: errors) : null,
                    trailing: IconButton(
                      tooltip: filtersExpanded
                          ? 'Replier les filtres'
                          : 'Déplier les filtres',
                      icon: Icon(
                        filtersExpanded ? Icons.expand_less : Icons.expand_more,
                        size: 18,
                      ),
                      visualDensity: VisualDensity.compact,
                      onPressed: () => ref
                          .read(sidebarFiltersExpandedProvider.notifier)
                          .toggle(),
                    ),
                  ),
                  // Sous-filtres Téléchargements (sauf « Tous » =
                  // entrée parente) — repliables via le chevron.
                  if (filtersExpanded)
                    for (final f in DownloadFilter.values.skip(1))
                      _FilterItem(
                        filter: f,
                        collapsed: collapsed,
                        icon: _filterIcons[f],
                        selected:
                            currentPath == '/downloads' &&
                            currentQuery['f'] == f.queryKey,
                        count: downloads?.where(f.matches).length,
                      ),
                  _NavItem(
                    d: kNavCatalog[1],
                    collapsed: collapsed,
                    selected: currentPath == '/search',
                  ),
                  if (!collapsed)
                    const Divider(height: AppSpacing.lg)
                  else
                    const _GroupLabel('Système'),
                  _NavItem(
                    d: kNavCatalog[3],
                    collapsed: collapsed,
                    selected: currentPath == '/settings',
                  ),
                  _NavItem(
                    d: kNavCatalog[2],
                    collapsed: collapsed,
                    selected: currentPath == '/diagnostic',
                  ),
                ],
              ),
            ),
            const Divider(height: 1),
            _DaemonFooter(collapsed: collapsed),
            Align(
              alignment: collapsed ? Alignment.center : Alignment.centerRight,
              child: IconButton(
                tooltip: collapsed ? 'Déplier la sidebar' : 'Replier en rail',
                icon: Icon(
                  collapsed ? Icons.chevron_right : Icons.chevron_left,
                  size: 20,
                ),
                onPressed: () =>
                    ref.read(sidebarCollapsedProvider.notifier).toggle(),
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
  const _NavItem({
    required this.d,
    required this.selected,
    this.collapsed = false,
    this.badge,
    this.trailing,
  });

  final NavDestinationSpec d;
  final bool selected;

  /// Mode rail : icône seule + tooltip sur le label.
  final bool collapsed;

  /// Badge en bout d'item (ex. compteur d'erreurs).
  final Widget? badge;

  /// Contrôle supplémentaire en bout d'item (ex. chevron replier).
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    return _SidebarItem(
      icon: selected ? d.selectedIcon : d.icon,
      label: d.label,
      collapsed: collapsed,
      selected: selected,
      trailing: badge != null || trailing != null
          ? Row(mainAxisSize: MainAxisSize.min, children: [?badge, ?trailing])
          : null,
      onTap: () => context.go(d.path),
    );
  }
}

/// Item de sous-filtre indenté avec icône et compteur badge.
class _FilterItem extends StatelessWidget {
  const _FilterItem({
    required this.filter,
    required this.selected,
    this.collapsed = false,
    this.icon,
    this.count,
  });

  final DownloadFilter filter;
  final bool selected;
  final bool collapsed;
  final IconData? icon;
  final int? count;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: EdgeInsets.only(left: collapsed ? 0 : AppSpacing.lg),
      child: _SidebarItem(
        icon: icon,
        label: filter.label,
        collapsed: collapsed,
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
    this.collapsed = false,
    this.icon,
    this.trailing,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;

  /// Mode rail : icône centrée, label masqué, `Tooltip` au survol.
  final bool collapsed;
  final IconData? icon;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final color = selected ? scheme.onPrimaryContainer : scheme.onSurface;
    if (collapsed) {
      return Padding(
        padding: const EdgeInsets.only(bottom: 2),
        child: Tooltip(
          message: label,
          child: InkWell(
            borderRadius: BorderRadius.circular(24),
            onTap: onTap,
            child: Container(
              decoration: BoxDecoration(
                color: selected ? scheme.primaryContainer : Colors.transparent,
                borderRadius: BorderRadius.circular(24),
              ),
              padding: const EdgeInsets.all(AppSpacing.sm - 2),
              child: Center(child: Icon(icon, size: 20, color: color)),
            ),
          ),
        ),
      );
    }
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

/// Débits globaux de session (↓/↑ temps réel) dans l'en-tête de la
/// sidebar — même source `totalSpeedsProvider` que la barre d'état.
class _SpeedsRow extends ConsumerWidget {
  const _SpeedsRow();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final speeds = ref.watch(totalSpeedsProvider);
    final theme = Theme.of(context);
    final style = theme.textTheme.bodySmall;
    return Row(
      children: [
        Expanded(
          child: Row(
            children: [
              Icon(
                Icons.arrow_downward,
                size: 13,
                color: theme.colorScheme.primary,
              ),
              const SizedBox(width: 2),
              Expanded(
                child: Text(
                  ByteFormatter.formatRate(speeds.down),
                  style: style,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        ),
        Expanded(
          child: Row(
            children: [
              Icon(
                Icons.arrow_upward,
                size: 13,
                color: theme.colorScheme.tertiary,
              ),
              const SizedBox(width: 2),
              Expanded(
                child: Text(
                  ByteFormatter.formatRate(speeds.up),
                  style: style,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Pied de sidebar : pastille de connexion SSE + version du daemon
/// (`GET /api/versioning/versions` → `current`). Réduit à la pastille
/// en mode rail.
class _DaemonFooter extends ConsumerWidget {
  const _DaemonFooter({required this.collapsed});

  final bool collapsed;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final theme = Theme.of(context);
    final connected = ref.watch(sseConnectedProvider).value ?? false;
    final version = ref.watch(versionsProvider).value?['current'];
    final dot = Icon(
      Icons.circle,
      size: 8,
      color: connected ? Colors.green : theme.colorScheme.error,
    );
    if (collapsed) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: AppSpacing.xs),
        child: Tooltip(
          message: connected
              ? 'Daemon connecté${version != null ? ' · $version' : ''}'
              : 'Daemon injoignable',
          child: Center(child: dot),
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: AppSpacing.md,
        vertical: AppSpacing.xs,
      ),
      child: Row(
        children: [
          dot,
          const SizedBox(width: AppSpacing.xs),
          Expanded(
            child: Text(
              connected
                  ? 'Daemon${version != null ? ' $version' : ''}'
                  : 'Daemon injoignable',
              style: theme.textTheme.bodySmall,
              overflow: TextOverflow.ellipsis,
            ),
          ),
        ],
      ),
    );
  }
}
