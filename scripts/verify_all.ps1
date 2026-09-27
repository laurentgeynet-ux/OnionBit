# verify_all.ps1 — validation complete du workspace avant de considerer
# une etape terminee. Cf. AGENTS.md, section "Commandes de validation".
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\verify_all.ps1

$ErrorActionPreference = "Stop"

Write-Host "== cargo check (workspace, tous les targets/features) ==" -ForegroundColor Cyan
cargo check --workspace --all-targets --all-features

Write-Host "== cargo clippy (workspace, warnings interdits) ==" -ForegroundColor Cyan
cargo clippy --workspace --all-targets --all-features -- -D warnings

Write-Host "== cargo fmt --check ==" -ForegroundColor Cyan
cargo fmt --all -- --check

Write-Host "== cargo test (workspace) ==" -ForegroundColor Cyan
cargo test --workspace --all-features

Write-Host "Validation complete OK." -ForegroundColor Green
