// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/material.dart';

import '../design/design_tokens.dart';
import '../l10n/l10n_ext.dart';

/// Paire « mot de passe + confirmation » pour la **création** d'un
/// secret chiffré (at-rest, export `OBID`) : la saisie masquée rend une
/// coquille invisible, la double entrée l'élimine. Concordance vérifiée
/// en continu — `onChanged` remonte le secret (chaîne vide = « sans mot
/// de passe ») ou `null` tant que les deux champs divergent, ce qui
/// permet au parent de griser le bouton de validation. À utiliser
/// partout où un mot de passe est *choisi* ; les dialogues qui en
/// *demandent* un existant (unlock, import, restauration) gardent un
/// champ unique — la confirmation n'y aurait aucun sens.
class PasswordPairField extends StatefulWidget {
  const PasswordPairField({
    super.key,
    required this.newLabel,
    this.helperText,
    this.enabled = true,
    this.onChanged,
    this.onSubmitted,
  });

  /// Libellé du premier champ (« Choose an at-rest password »…).
  final String newLabel;

  /// Indice sous le premier champ (wizard : « optionnel, chiffre la
  /// graine »…).
  final String? helperText;

  /// Faux pendant qu'une requête est en vol — verrouille les 2 champs.
  final bool enabled;

  /// `null` = divergence ; `''` = les deux vides (aucun secret) ;
  /// sinon le secret confirmé. Rappelé à chaque frappe.
  final ValueChanged<String?>? onChanged;

  /// Entrée depuis le clavier dans l'un des deux champs — invoqué
  /// uniquement quand les champs concordent, avec le secret.
  final ValueChanged<String>? onSubmitted;

  @override
  State<PasswordPairField> createState() => _PasswordPairFieldState();
}

class _PasswordPairFieldState extends State<PasswordPairField> {
  final _password = TextEditingController();
  final _confirm = TextEditingController();

  bool get _matching => _password.text == _confirm.text;

  void _sync() {
    setState(() {});
    widget.onChanged?.call(_matching ? _password.text : null);
  }

  void _submit(String _) {
    if (_matching) widget.onSubmitted?.call(_password.text);
  }

  @override
  void dispose() {
    _password.dispose();
    _confirm.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextField(
          controller: _password,
          obscureText: true,
          enabled: widget.enabled,
          decoration: InputDecoration(
            labelText: widget.newLabel,
            helperText: widget.helperText,
            prefixIcon: const Icon(Icons.lock_outline),
            border: const OutlineInputBorder(),
          ),
          onChanged: (_) => _sync(),
          onSubmitted: _submit,
        ),
        const SizedBox(height: AppSpace.xs),
        TextField(
          controller: _confirm,
          obscureText: true,
          enabled: widget.enabled,
          decoration: InputDecoration(
            labelText: l10n.identityPasswordConfirm,
            prefixIcon: const Icon(Icons.lock_outline),
            border: const OutlineInputBorder(),
            errorText: _matching || _confirm.text.isEmpty
                ? null
                : l10n.identityPasswordMismatch,
          ),
          onChanged: (_) => _sync(),
          onSubmitted: _submit,
        ),
      ],
    );
  }
}
