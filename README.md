# OnionBit

Portage en Rust du daemon [Tribler](https://github.com/Tribler/tribler)
(client BitTorrent avec réseau d'anonymisation IPv8/TunnelCommunity), avec
interface Flutter multiplateforme (Windows x64/arm64, Linux, macOS,
Android, iOS, Web) dans `app/`.

Version : **0.3.1-alpha** — l'interop avec Tribler 8.x est validée sur le
banc (téléchargements anonymes via circuits, hidden seeding, résilience
aux kills), cf. [`docs/CHANGELOG.md`](docs/CHANGELOG.md).

## Documentation

- [`docs/plans/plan_faisabilite.md`](docs/plans/plan_faisabilite.md) —
  analyse de faisabilité, risques, décisions arbitrées.
- [`docs/plans/roadmap.md`](docs/plans/roadmap.md) — plan d'implémentation
  détaillé, étape par étape (source de vérité de l'avancement).
- [`docs/architecture/architecture.md`](docs/architecture/architecture.md) —
  vue d'ensemble de la clean architecture.
- [`docs/architecture/decisions/`](docs/architecture/decisions/) — décisions
  d'architecture actées (ADRs).
- [`docs/reference_tribler/`](docs/reference_tribler/) — correspondance
  entre les modules Python de Tribler et les crates Rust.
- [`AGENTS.md`](AGENTS.md) — règles pour agents IA (workflow, conventions).

## Structure

Workspace Cargo, 12 crates sous `crates/` (`onionbit-format`,
`onionbit-crypto`, `onionbit-bittorrent`, `onionbit-ipv8`, `onionbit-tunnel`,
`onionbit-core`, `onionbit-db`, `onionbit-network-policy`, `onionbit-api`,
`onionbit-cli`, `onionbit-daemon`, `onionbit-test-support`). Détail des
responsabilités dans [`docs/architecture/architecture.md`](docs/architecture/architecture.md).

## Validation

```powershell
powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\verify_all.ps1
```

## Licence

GPL-3.0-or-later — voir [`LICENSE`](LICENSE). Ce projet porte
l'architecture et le comportement du logiciel Tribler (GPL-3.0) ; voir
[ADR-0003](docs/architecture/decisions/0003-licence-gpl3.md).
