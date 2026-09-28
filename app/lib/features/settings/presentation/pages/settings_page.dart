import 'package:flutter/material.dart';

import '../widgets/appearance_section.dart';
import '../widgets/connection_section.dart';
import '../widgets/daemon_section.dart';
import '../widgets/downloads_section.dart';

/// Page « Réglages » — apparence (thème persisté), connexion au
/// daemon, état, téléchargements et arrêt.
class SettingsPage extends StatelessWidget {
  const SettingsPage({super.key});

  @override
  Widget build(BuildContext context) {
    return ListView(
      children: const [
        AppearanceSection(),
        DownloadsSection(),
        ConnectionSection(),
        DaemonSection(),
      ],
    );
  }
}
