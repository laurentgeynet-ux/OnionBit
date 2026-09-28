import '../../../core/api/api_client.dart';
import '../domain/settings_repository.dart';

/// Implémentation REST du dépôt réglages.
class RestSettingsRepository implements SettingsRepository {
  RestSettingsRepository(this._api);

  final ApiClient _api;

  @override
  Future<Map<String, dynamic>> get() async =>
      (await _api.get('/settings') as Map<String, dynamic>)['settings']
          as Map<String, dynamic>? ??
      const {};

  @override
  Future<void> shutdown() => _api.put('/shutdown');
}
