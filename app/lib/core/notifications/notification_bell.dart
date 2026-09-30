import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../theme/app_theme.dart';
import 'app_notification.dart';
import 'notifications_provider.dart';

/// Cloche de la `TopBar` : badge de non-lues + `MenuAnchor` listant
/// les notifications récentes (marquer tout lu / vider).
class NotificationBell extends ConsumerWidget {
  const NotificationBell({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final unread = ref.watch(unreadNotificationsProvider);
    final items = ref.watch(notificationsProvider);
    return MenuAnchor(
      alignmentOffset: const Offset(0, 4),
      menuChildren: [
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: AppSpacing.md,
            vertical: AppSpacing.xs,
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                'Notifications',
                style: Theme.of(context).textTheme.titleSmall,
              ),
              const SizedBox(width: AppSpacing.md),
              TextButton(
                onPressed: unread == 0
                    ? null
                    : () => ref
                          .read(notificationsProvider.notifier)
                          .markAllRead(),
                child: const Text('Tout lu'),
              ),
              TextButton(
                onPressed: items.isEmpty
                    ? null
                    : () => ref.read(notificationsProvider.notifier).clear(),
                child: const Text('Vider'),
              ),
            ],
          ),
        ),
        const Divider(height: 1),
        if (items.isEmpty)
          const Padding(
            padding: EdgeInsets.all(AppSpacing.md),
            child: Text('Aucune notification'),
          )
        else
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 360, maxHeight: 320),
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final n in items.take(50)) _NotificationTile(n: n),
                ],
              ),
            ),
          ),
      ],
      builder: (context, controller, _) => IconButton(
        tooltip: 'Notifications',
        icon: Badge(
          isLabelVisible: unread > 0,
          label: Text('$unread'),
          child: const Icon(Icons.notifications_outlined, size: 20),
        ),
        onPressed: () {
          if (controller.isOpen) {
            controller.close();
          } else {
            controller.open();
            ref.read(notificationsProvider.notifier).markAllRead();
          }
        },
      ),
    );
  }
}

class _NotificationTile extends StatelessWidget {
  const _NotificationTile({required this.n});

  final AppNotification n;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final color = switch (n.severity) {
      AppNotificationSeverity.info => scheme.primary,
      AppNotificationSeverity.success => Colors.green,
      AppNotificationSeverity.warning => scheme.tertiary,
      AppNotificationSeverity.error => scheme.error,
    };
    return ListTile(
      dense: true,
      leading: Icon(n.icon, size: 18, color: color),
      title: Text(n.title, overflow: TextOverflow.ellipsis),
      subtitle: n.message.isEmpty
          ? null
          : Text(n.message, overflow: TextOverflow.ellipsis, maxLines: 2),
      trailing: Text(
        '${n.timestamp.hour.toString().padLeft(2, '0')}:'
        '${n.timestamp.minute.toString().padLeft(2, '0')}',
        style: theme.textTheme.labelSmall?.copyWith(color: scheme.outline),
      ),
    );
  }
}
