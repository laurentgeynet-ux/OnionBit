// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../../layout/breakpoints.dart';

/// Layout liste+détail adaptatif (ADR-0021 §5) : deux panneaux côte à
/// côte à partir de [sidePaneBreakpoint] (par défaut `expanded`,
/// ≥ 1024 dp) ; en dessous, seul [list] est rendu — à l'appelant de
/// pousser une route/un panneau plein écran pour le détail (navigation
/// « push », pas de deuxième colonne possible à cette largeur).
///
/// Primitif réutilisable, pas encore consommé par un écran migré :
/// les téléchargements gardent leur panneau de détail existant
/// (`DownloadDetailPanel`, empilé sous la table — mieux adapté à un
/// contenu tabulaire large qu'une colonne étroite). Candidat naturel :
/// la messagerie (étape 74), conversations à gauche / fil à droite.
class AdaptiveListDetail extends StatelessWidget {
  const AdaptiveListDetail({
    super.key,
    required this.list,
    required this.detail,
    this.sidePaneBreakpoint = AppBreakpoint.expanded,
    this.listWidth = 360,
  });

  /// Volet liste — toujours rendu.
  final Widget list;

  /// Volet détail — rendu uniquement à partir de [sidePaneBreakpoint].
  /// L'appelant fournit son propre état vide (ex. `EmptyState`) quand
  /// rien n'est sélectionné ; ce primitif ne préjuge pas du contenu.
  final Widget detail;

  /// Palier à partir duquel les deux volets s'affichent côte à côte.
  final AppBreakpoint sidePaneBreakpoint;

  /// Largeur fixe du volet liste quand les deux panneaux sont visibles.
  final double listWidth;

  @override
  Widget build(BuildContext context) {
    final breakpoint = AppBreakpoints.of(MediaQuery.sizeOf(context).width);
    final sideBySide = breakpoint.index >= sidePaneBreakpoint.index;
    if (!sideBySide) return list;
    return Row(
      children: [
        SizedBox(width: listWidth, child: list),
        const VerticalDivider(width: 1),
        Expanded(child: detail),
      ],
    );
  }
}
