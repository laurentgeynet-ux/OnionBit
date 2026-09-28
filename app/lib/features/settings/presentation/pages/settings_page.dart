import 'package:flutter/material.dart';

import '../widgets/appearance_section.dart';
import '../widgets/connection_section.dart';
import '../widgets/daemon_section.dart';

/// Page « Réglages » — apparence (thème persisté), connexion au
/// daemon, état et arrêt.
class SettingsPage extends StatelessWidget {
  const SettingsPage({super.key});

  @override
  Widget build(BuildContext context) {
    return ListView(
      children: const [
        AppearanceSection(),
        ConnectionSection(),
        DaemonSection(),
      ],
    );
  }
}
