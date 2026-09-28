import 'package:flutter/material.dart';

import '../widgets/anonymity_section.dart';
import '../widgets/appearance_section.dart';
import '../widgets/automation_section.dart';
import '../widgets/bandwidth_section.dart';
import '../widgets/connection_section.dart';
import '../widgets/daemon_section.dart';
import '../widgets/downloads_section.dart';
import '../widgets/network_section.dart';
import '../widgets/queue_section.dart';
import '../widgets/seeding_section.dart';
import '../widgets/versioning_section.dart';

/// Page « Réglages » — apparence (thème persisté), téléchargements,
/// bande passante, file d'attente, seed, anonymat, réseau,
/// automatisation, mises à jour, connexion au daemon et arrêt.
class SettingsPage extends StatelessWidget {
  const SettingsPage({super.key});

  @override
  Widget build(BuildContext context) {
    return ListView(
      children: const [
        AppearanceSection(),
        DownloadsSection(),
        BandwidthSection(),
        QueueSection(),
        SeedingSection(),
        AnonymitySection(),
        NetworkSection(),
        AutomationSection(),
        VersioningSection(),
        ConnectionSection(),
        DaemonSection(),
      ],
    );
  }
}
