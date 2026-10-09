// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../di/providers.dart';
import '../l10n/l10n_ext.dart';
import '../notifications/notification_bell.dart';
import '../design/design_tokens.dart';

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
  final _controller = TextEditingController();
  Timer? _debounce;

  @override
  void dispose() {
    _debounce?.cancel();
    _controller.dispose();
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

  @override
  Widget build(BuildContext context) {
    // Si la requête change ailleurs (page recherche), le champ suit.
    ref.listen(searchQueryProvider, (_, q) {
      if (_controller.text != q) {
        _controller.value = TextEditingValue(
          text: q,
          selection: TextSelection.collapsed(offset: q.length),
        );
      }
    });
    return Container(
      height: TopBar.height,
      padding: const EdgeInsets.symmetric(horizontal: AppSpace.md),
      child: Row(
        children: [
          Expanded(
            child: TextField(
              controller: _controller,
              decoration: InputDecoration(
                hintText: context.l10n.searchHint,
                prefixIcon: const Icon(Icons.search),
                isDense: true,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(AppRadius.small),
                ),
                suffixIcon: _controller.text.isEmpty
                    ? null
                    : IconButton(
                        icon: const Icon(Icons.clear, size: 18),
                        onPressed: () {
                          _controller.clear();
                          ref.read(searchQueryProvider.notifier).set('');
                        },
                      ),
              ),
              onChanged: _onChanged,
              onSubmitted: _submit,
            ),
          ),
          const SizedBox(width: AppSpace.sm),
          const NotificationBell(),
        ],
      ),
    );
  }
}
