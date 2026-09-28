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
/// entrée Téléchargements + sous-filtres avec compteurs, puis
/// Rechercher / Réglages / Diagnostic.
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

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final scheme = Theme.of(context).colorScheme;
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
                padding: EdgeInsets.zero,
                children: [
                  _item(
                    context,
                    kNavCatalog[0],
                    selected:
                        currentPath == '/downloads' &&
                        currentQuery['f'] == null,
                  ),
                  // Sous-filtres Téléchargements (sauf « Tous » =
                  // entrée parente).
                  for (final f in DownloadFilter.values.skip(1))
                    _filterItem(context, ref, f),
                  _item(
                    context,
                    kNavCatalog[1],
                    selected: currentPath == '/search',
                  ),
                  const Divider(height: AppSpacing.lg),
                  _item(
                    context,
                    kNavCatalog[3],
                    selected: currentPath == '/settings',
                  ),
                  _item(
                    context,
                    kNavCatalog[2],
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

  Widget _item(
    BuildContext context,
    NavDestinationSpec d, {
    required bool selected,
  }) {
    final scheme = Theme.of(context).colorScheme;
    return ListTile(
      dense: true,
      selected: selected,
      selectedTileColor: scheme.primaryContainer,
      leading: Icon(selected ? d.selectedIcon : d.icon, size: 20),
      title: Text(d.label),
      onTap: () => context.go(d.path),
    );
  }

  Widget _filterItem(
    BuildContext context,
    WidgetRef ref,
    DownloadFilter filter,
  ) {
    final scheme = Theme.of(context).colorScheme;
    final selected =
        currentPath == '/downloads' && currentQuery['f'] == filter.queryKey;
    final downloads = ref.watch(downloadsProvider).value;
    final count = downloads?.where(filter.matches).length;
    return ListTile(
      dense: true,
      contentPadding: const EdgeInsets.only(left: AppSpacing.xl + 8),
      selected: selected,
      selectedTileColor: scheme.primaryContainer,
      title: Text(filter.label),
      trailing: count == null
          ? null
          : Text('$count', style: Theme.of(context).textTheme.bodySmall),
      onTap: () => context.go('/downloads?f=${filter.queryKey}'),
    );
  }
}
