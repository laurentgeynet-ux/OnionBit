# Tribler-Rust-Torrent

Portage en Rust du daemon [Tribler](https://github.com/Tribler/tribler)
(client BitTorrent avec réseau d'anonymisation IPv8/TunnelCommunity), avec
une future interface Flutter multiplateforme (Windows x64/arm64, Linux,
macOS, Android, iOS, Web).

**Backend d'abord** : aucune ligne d'UI ne sera écrite avant que le daemon
soit validé à 100 % sur les fonctionnalités listées dans
[`docs/plans/roadmap.md`](docs/plans/roadmap.md).

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

Workspace Cargo, 12 crates sous `crates/` (`tribler-format`,
`tribler-crypto`, `tribler-bittorrent`, `tribler-ipv8`, `tribler-tunnel`,
`tribler-core`, `tribler-db`, `tribler-network-policy`, `tribler-api`,
`tribler-cli`, `tribler-daemon`, `tribler-test-support`). Détail des
responsabilités dans [`docs/architecture/architecture.md`](docs/architecture/architecture.md).

## Validation

```powershell
powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\verify_all.ps1
```

## Licence

GPL-3.0-or-later — voir [`LICENSE`](LICENSE). Ce projet porte
l'architecture et le comportement du logiciel Tribler (GPL-3.0) ; voir
[ADR-0003](docs/architecture/decisions/0003-licence-gpl3.md).
