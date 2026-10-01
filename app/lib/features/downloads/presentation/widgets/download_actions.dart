// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../core/l10n/l10n_ext.dart';
import '../../../../core/theme/app_theme.dart';
import '../../../../core/utils/byte_formatter.dart';
import '../../domain/download.dart';
import '../providers/downloads_providers.dart';

/// Dialogues d'action d'un téléchargement : limites de débit
/// individuelles, ratio de seed, déplacement du stockage. Partagés
/// entre le menu contextuel de la liste et le panneau de détail.

/// Message d'erreur uniforme des actions de téléchargement.
void showDownloadError(BuildContext context, String label, Object error) {
  ScaffoldMessenger.of(
    context,
  ).showSnackBar(SnackBar(content: Text('$label : $error')));
}

/// Dialogue « Limites de débit » — Ko/s, champ vide ou 0 = illimité.
/// Le backend ignore les valeurs 0 (`if upload_limit := …` Python) ;
/// « illimité » s'obtient en effaçant la limite côté fichier de
/// configuration — on envoie donc la valeur saisie et on documente
/// que vider = garder l'illimité actuel.
Future<void> showRateLimitsDialog(BuildContext context, Download d) async {
  final up = TextEditingController(
    text: d.uploadLimit > 0 ? '${d.uploadLimit ~/ 1024}' : '',
  );
  final down = TextEditingController(
    text: d.downloadLimit > 0 ? '${d.downloadLimit ~/ 1024}' : '',
  );
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(ctx.l10n.rateLimits),
      content: SizedBox(
        width: 360,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: down,
              keyboardType: TextInputType.number,
              decoration: InputDecoration(
                labelText: ctx.l10n.rateFieldLabel(ctx.l10n.bwDownloadLabel),
                hintText: ctx.l10n.emptyUnlimited,
                prefixIcon: const Icon(Icons.arrow_downward),
              ),
            ),
            const SizedBox(height: AppSpacing.sm),
            TextField(
              controller: up,
              keyboardType: TextInputType.number,
              decoration: InputDecoration(
                labelText: ctx.l10n.rateFieldLabel(ctx.l10n.bwUploadLabel),
                hintText: ctx.l10n.emptyUnlimited,
                prefixIcon: const Icon(Icons.arrow_upward),
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(false),
          child: Text(ctx.l10n.cancel),
        ),
        FilledButton(
          onPressed: () => Navigator.of(ctx).pop(true),
          child: Text(ctx.l10n.apply),
        ),
      ],
    ),
  );
  if (ok != true || !context.mounted) return;
  int? parseKb(TextEditingController c) {
    final v = int.tryParse(c.text.trim());
    return v == null ? null : v * 1024;
  }

  // `0`/vide → `null` : le backend ignore les valeurs nulles (règle
  // Python « walrus »), la limite effective reste donc l'illimité ou
  // la valeur précédente — comportement documenté du PATCH.
  final container = ProviderScope.containerOf(context);
  try {
    await container
        .read(downloadsProvider.notifier)
        .setRateLimits(
          d.infohash,
          uploadLimit: parseKb(up),
          downloadLimit: parseKb(down),
        );
  } catch (e) {
    if (context.mounted) {
      showDownloadError(context, context.l10n.actLimits, e);
    }
  }
}

/// Dialogue « Ratio de seed » — borne individuelle ou retour au
/// défaut (`seeding_ratio_default`).
Future<void> showSeedingRatioDialog(
  BuildContext context,
  Download d,
) async {
  final controller = TextEditingController(
    text: d.seedingRatio > 0 ? d.seedingRatio.toString() : '',
  );
  var useDefault = d.seedingRatio <= 0;
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, setState) => AlertDialog(
        title: Text(ctx.l10n.ratioTitle),
        content: SizedBox(
          width: 360,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              SwitchListTile(
                value: useDefault,
                onChanged: (v) => setState(() => useDefault = v),
                title: Text(ctx.l10n.useDefaultSetting),
                contentPadding: EdgeInsets.zero,
                dense: true,
              ),
              TextField(
                controller: controller,
                enabled: !useDefault,
                keyboardType: const TextInputType.numberWithOptions(
                  decimal: true,
                ),
                decoration: InputDecoration(
                  labelText: ctx.l10n.targetRatio,
                  hintText: 'ex. 2.0',
                  prefixIcon: const Icon(Icons.balance),
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(ctx.l10n.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text(ctx.l10n.apply),
          ),
        ],
      ),
    ),
  );
  if (ok != true || !context.mounted) return;
  final container = ProviderScope.containerOf(context);
  final notifier = container.read(downloadsProvider.notifier);
  try {
    if (useDefault) {
      await notifier.resetSeedingRatio(d.infohash);
    } else {
      final ratio = double.tryParse(controller.text.trim());
      if (ratio == null || ratio <= 0) {
        if (context.mounted) {
          showDownloadError(
            context,
            context.l10n.actRatio,
            context.l10n.invalidValue,
          );
        }
        return;
      }
      await notifier.setSeedingRatio(d.infohash, ratio);
    }
  } catch (e) {
    if (context.mounted) {
      showDownloadError(context, context.l10n.actRatio, e);
    }
  }
}

/// Dialogue « Déplacer le stockage » (`state=move_storage` +
/// `dest_dir`, `completed_dir` optionnel).
Future<void> showMoveStorageDialog(BuildContext context, Download d) async {
  final dest = TextEditingController(text: d.destination);
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(ctx.l10n.moveTitle),
      content: SizedBox(
        width: 420,
        child: TextField(
          controller: dest,
          autofocus: true,
          decoration: InputDecoration(
            labelText: ctx.l10n.newDest,
            prefixIcon: const Icon(Icons.drive_file_move_outlined),
            suffixIcon: IconButton(
              tooltip: ctx.l10n.browse,
              icon: const Icon(Icons.folder_open),
              onPressed: () async {
                final dir = await getDirectoryPath();
                if (dir != null) dest.text = dir;
              },
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(false),
          child: Text(ctx.l10n.cancel),
        ),
        FilledButton(
          onPressed: () => Navigator.of(ctx).pop(true),
          child: Text(ctx.l10n.moveConfirm),
        ),
      ],
    ),
  );
  if (ok != true || !context.mounted) return;
  final target = dest.text.trim();
  if (target.isEmpty || target == d.destination) return;
  final container = ProviderScope.containerOf(context);
  try {
    await container
        .read(downloadsProvider.notifier)
        .moveStorage(d.infohash, destination: target);
    if (context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(context.l10n.moveRequested)),
      );
    }
  } catch (e) {
    if (context.mounted) {
      showDownloadError(context, context.l10n.actMove, e);
    }
  }
}

/// Libellé court des limites individuelles (« — » = illimité).
String formatLimits(Download d) {
  String f(int v) => v > 0 ? ByteFormatter.formatRate(v) : '∞';
  return '↓ ${f(d.downloadLimit)} · ↑ ${f(d.uploadLimit)}';
}
