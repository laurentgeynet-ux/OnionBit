import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../../features/search/presentation/providers/search_providers.dart';
import '../di/providers.dart';
import '../theme/app_theme.dart';

/// Barre du haut : champ de recherche global (locale + distante).
/// Taper (debounce 300 ms) met à jour `searchQueryProvider` et
/// navigue vers `/search` — vider le champ ramène aux populaires.
class TopBar extends ConsumerStatefulWidget {
  const TopBar({super.key});

  static const double height = 56;

  @override
  ConsumerState<TopBar> createState() => _TopBarState();
}

class _TopBarState extends ConsumerState<TopBar> {
  TextEditingController? _controller;
  Timer? _debounce;
  Timer? _acDebounce;
  Completer<Iterable<String>>? _pendingAc;

  @override
  void dispose() {
    _debounce?.cancel();
    _acDebounce?.cancel();
    _pendingAc?.complete(const Iterable<String>.empty());
    // Le controller est possede par `Autocomplete` — il le dispose
    // lui-meme, ne pas le toucher ici.
    super.dispose();
  }

  void _submit(String value) {
    ref.read(searchQueryProvider.notifier).set(value.trim());
    if (value.trim().isNotEmpty && mounted) {
      context.go('/search');
    }
  }

  void _onChanged(String value) {
    setState(() {}); // Rafraîchit le suffixe « effacer ».
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 300), () {
      _submit(value);
    });
  }

  /// Suggestions FTS du daemon (`GET /metadata/search/completions`),
  /// debouncées — l'autocomplétion ne doit jamais bloquer la frappe.
  Future<Iterable<String>> _completions(String text) {
    _acDebounce?.cancel();
    _pendingAc?.complete(const Iterable<String>.empty());
    final c = Completer<Iterable<String>>();
    _pendingAc = c;
    _acDebounce = Timer(const Duration(milliseconds: 250), () async {
      if (c.isCompleted) return;
      try {
        c.complete(
          await ref.read(searchRepositoryProvider).completions(text),
        );
      } catch (_) {
        if (!c.isCompleted) c.complete(const Iterable<String>.empty());
      }
    });
    return c.future;
  }

  @override
  Widget build(BuildContext context) {
    // Si la requête change ailleurs (page recherche), le champ suit.
    ref.listen(searchQueryProvider, (_, q) {
      final c = _controller;
      if (c != null && c.text != q) {
        c.value = TextEditingValue(
          text: q,
          selection: TextSelection.collapsed(offset: q.length),
        );
      }
    });
    return Container(
      height: TopBar.height,
      padding: const EdgeInsets.symmetric(horizontal: AppSpacing.md),
      child: Row(
        children: [
          Expanded(
            child: Autocomplete<String>(
              optionsBuilder: (text) {
                final q = text.text.trim();
                if (q.isEmpty) return const Iterable<String>.empty();
                return _completions(q);
              },
              onSelected: _submit,
              optionsViewBuilder: (context, onSelected, options) => Align(
                alignment: Alignment.topLeft,
                child: Material(
                  elevation: 4,
                  borderRadius: BorderRadius.circular(AppRadii.small),
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(
                      maxWidth: 480,
                      maxHeight: 280,
                    ),
                    child: ListView.builder(
                      shrinkWrap: true,
                      padding: EdgeInsets.zero,
                      itemCount: options.length,
                      itemBuilder: (context, i) {
                        final o = options.elementAt(i);
                        return ListTile(
                          dense: true,
                          leading: const Icon(Icons.search, size: 18),
                          title: Text(o, overflow: TextOverflow.ellipsis),
                          onTap: () => onSelected(o),
                        );
                      },
                    ),
                  ),
                ),
              ),
              fieldViewBuilder:
                  (context, controller, focusNode, onFieldSubmitted) {
                    _controller = controller;
                    return TextField(
                      controller: controller,
                      focusNode: focusNode,
                      decoration: InputDecoration(
                        hintText: 'Rechercher du contenu…',
                        prefixIcon: const Icon(Icons.search),
                        isDense: true,
                        border: OutlineInputBorder(
                          borderRadius: BorderRadius.circular(
                            AppRadii.small,
                          ),
                        ),
                        suffixIcon: controller.text.isEmpty
                            ? null
                            : IconButton(
                                icon: const Icon(Icons.clear, size: 18),
                                onPressed: () {
                                  controller.clear();
                                  ref
                                      .read(searchQueryProvider.notifier)
                                      .set('');
                                },
                              ),
                      ),
                      onChanged: _onChanged,
                      onSubmitted: (_) {
                        onFieldSubmitted();
                        _submit(controller.text);
                      },
                    );
                  },
            ),
          ),
        ],
      ),
    );
  }
}
