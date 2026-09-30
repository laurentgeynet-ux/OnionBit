import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/theme/app_theme.dart';
import '../providers/settings_providers.dart';
import 'settings_section.dart';
import 'settings_defaults.dart';

/// Section « Automatisation » — dossier surveillé
/// (`watch_folder/*`) et flux RSS (`rss/*` + `PUT /api/rss` pour
/// application à chaud). Affiche aussi les derniers items RSS
/// découverts par le daemon.
class AutomationSection extends ConsumerStatefulWidget {
  const AutomationSection({super.key});

  @override
  ConsumerState<AutomationSection> createState() => _AutomationSectionState();
}

class _AutomationSectionState extends ConsumerState<AutomationSection> {
  late final _deferred = DeferredSection(ref, 'automation');
  final _watchDir = TextEditingController();
  final _watchInterval = TextEditingController();
  final _newFeed = TextEditingController();
  bool _watchEnabled = false;
  bool _rssEnabled = false;
  List<String> _feeds = [];
  bool _initialized = false;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _deferred.attach(save: _save, discard: _discard);
  }

  @override
  void dispose() {
    _deferred.detach();
    _watchDir.dispose();
    _watchInterval.dispose();
    _newFeed.dispose();
    super.dispose();
  }

  void _sync(Map<String, dynamic> settings) {
    if (_initialized) return;
    _watchEnabled = settingsBool(settings, const ['watch_folder', 'enabled']);
    _watchDir.text = settingsString(settings, const [
      'watch_folder',
      'directory',
    ]);
    _watchInterval.text =
        '${settingsDouble(settings, const ['watch_folder', 'check_interval'], def: 10).round()}';
    _rssEnabled = settingsBool(settings, const ['rss', 'enabled'], def: true);
    _feeds = [
      for (final u
          in (settingsLeaf(settings, const ['rss', 'urls']) as List?)
                  ?.whereType<Object>() ??
              const [])
        '$u',
    ];
    _initialized = true;
  }

  void _discard() => setState(() {
    _initialized = false;
    _deferred.markClean();
  });

  Future<void> _save() async {
    setState(() => _saving = true);
    final repo = ref.read(settingsRepositoryProvider);
    try {
      await repo.update({
        'watch_folder': {
          'enabled': _watchEnabled,
          'directory': _watchDir.text.trim(),
          'check_interval': double.tryParse(_watchInterval.text.trim()) ?? 10,
        },
        'rss': {'enabled': _rssEnabled, 'urls': _feeds},
      });
      // Application à chaud des flux (l'équivalent Python : le PUT
      // recrée/met à jour le RssManager immédiatement, sans attendre
      // le prochain démarrage).
      await repo.setRssFeeds(_rssEnabled ? _feeds : const []);
      ref.invalidate(daemonSettingsProvider);
      ref.invalidate(rssItemsProvider);
      _deferred.markClean();
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Automatisation enregistrée')),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text('Erreur : $e')));
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  void _addFeed() {
    final url = _newFeed.text.trim();
    if (url.isEmpty || _feeds.contains(url)) return;
    setState(() {
      _feeds.add(url);
      _newFeed.clear();
      _deferred.markDirty();
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SettingsSection(
      icon: Icons.smart_button_outlined,
      title: 'Automatisation',
      sectionId: 'automation',
      defaults: kAutomationDefaults,
      child: (context, settings) {
        _sync(settings);
        final items = ref.watch(rssItemsProvider).value ?? const [];
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Dossier surveillé', style: theme.textTheme.labelMedium),
            SwitchListTile(
              value: _watchEnabled,
              onChanged: (v) => setState(() {
                _watchEnabled = v;
                _deferred.markDirty();
              }),
              title: const Text('Surveiller un dossier'),
              subtitle: const Text(
                'Les fichiers .torrent déposés dedans sont ajoutés '
                'automatiquement. Défaut : désactivé.',
              ),
              contentPadding: EdgeInsets.zero,
              dense: true,
            ),
            if (_watchEnabled) ...[
              TextField(
                controller: _watchDir,
                onChanged: (_) => _deferred.markDirty(),
                decoration: InputDecoration(
                  labelText: 'Dossier à surveiller',
                  prefixIcon: const Icon(Icons.folder_outlined),
                  suffixIcon: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const KeyInfoIcon(['watch_folder', 'directory']),
                      IconButton(
                        tooltip: 'Parcourir…',
                        icon: const Icon(Icons.folder_open),
                        onPressed: () async {
                          final dir = await getDirectoryPath();
                          if (dir != null) {
                            setState(() {
                              _watchDir.text = dir;
                              _deferred.markDirty();
                            });
                          }
                        },
                      ),
                    ],
                  ),
                ),
              ),
              const SizedBox(height: AppSpacing.sm),
              TextField(
                controller: _watchInterval,
                onChanged: (_) => _deferred.markDirty(),
                keyboardType: TextInputType.number,
                decoration: const InputDecoration(
                  labelText: 'Intervalle de scan (s) — défaut : 10',
                  isDense: true,
                  suffixIcon: KeyInfoIcon(['watch_folder', 'check_interval']),
                ),
              ),
            ],
            const Divider(height: AppSpacing.lg),
            Text('Flux RSS', style: theme.textTheme.labelMedium),
            SwitchListTile(
              value: _rssEnabled,
              onChanged: (v) => setState(() {
                _rssEnabled = v;
                _deferred.markDirty();
              }),
              title: const Text('Watchers RSS actifs'),
              subtitle: const Text('Défaut : activé (aucun flux).'),
              contentPadding: EdgeInsets.zero,
              dense: true,
            ),
            if (_rssEnabled) ...[
              for (final url in _feeds)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(Icons.rss_feed, size: 18),
                  title: Text(url, overflow: TextOverflow.ellipsis),
                  trailing: IconButton(
                    icon: const Icon(Icons.remove_circle_outline, size: 18),
                    tooltip: 'Retirer',
                    onPressed: () => setState(() {
                      _feeds.remove(url);
                      _deferred.markDirty();
                    }),
                  ),
                ),
              Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: _newFeed,
                      decoration: const InputDecoration(
                        labelText: 'URL du flux',
                        hintText: 'https://exemple.com/feed.xml',
                        isDense: true,
                        suffixIcon: KeyInfoIcon(['rss', 'urls']),
                      ),
                      onSubmitted: (_) => _addFeed(),
                    ),
                  ),
                  IconButton(
                    icon: const Icon(Icons.add),
                    tooltip: 'Ajouter le flux',
                    onPressed: _addFeed,
                  ),
                ],
              ),
              if (items.isNotEmpty) ...[
                const SizedBox(height: AppSpacing.sm),
                Text(
                  'Derniers items découverts',
                  style: theme.textTheme.labelMedium,
                ),
                for (final i in items.take(8))
                  ListTile(
                    dense: true,
                    contentPadding: EdgeInsets.zero,
                    leading: const Icon(Icons.article_outlined, size: 16),
                    title: Text(
                      '${i['title'] ?? i['link'] ?? ''}',
                      overflow: TextOverflow.ellipsis,
                    ),
                    subtitle: Text(
                      '${i['feed_url'] ?? ''}',
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ),
              ],
            ],
            const SizedBox(height: AppSpacing.sm),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton.icon(
                onPressed: _saving ? null : _save,
                icon: const Icon(Icons.save),
                label: const Text('Enregistrer'),
              ),
            ),
          ],
        );
      },
    );
  }
}
