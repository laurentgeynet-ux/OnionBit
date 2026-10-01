# Usage ponctuel : cree la release GitHub + upload l'asset zip.
# Token lu depuis l'env utilisateur GITHUB_TOKEN (jamais affiche).
$t = [Environment]::GetEnvironmentVariable('GITHUB_TOKEN','User')
if (-not $t) { throw 'GITHUB_TOKEN absent' }
$h = @{ Authorization = "Bearer $t"; 'X-GitHub-Api-Version' = '2022-11-28' }
$repo = 'laurentgeynet-ux/OnionBit'

$notes = @'
## OnionBit 0.3.1-alpha - first public alpha

Native Rust port of [Tribler](https://github.com/Tribler/tribler): anonymous BitTorrent over multi-hop onion circuits (IPv8 overlay + TunnelCommunity port), with a Flutter desktop UI.

> **Alpha - expect rough edges.** Validated against the real Tribler 8.x network on the interop testbench (anonymous downloads, hidden seeding, kill/restart resilience). Not yet recommended for high-stakes anonymity.

### Windows x64 package

`OnionBit-0.3.1-alpha-windows-x64.zip` - unzip anywhere, run `onionbit_ui.exe` (it spawns the bundled `onionbit-daemon.exe` automatically; `onionbit-cli.exe` included for CLI control). First launch creates `%APPDATA%\onionbit\` (state, SQLite db, logs).

**SHA-256:** `73AB8F0421FC70E1AF5170BB3E4C7F9B0ED46DB991CA9D163D175127E742CC4B`

Other platforms (Linux, macOS, Android, iOS, Web): build from source - see `docs/BUILDING.md`.

### Highlights

- Multi-hop onion-routed downloads and hidden seeding, interop-verified with Tribler 8.4.3
- REST + SSE control plane on loopback (axum), `onionbit-cli` for scripting
- librqbit engine: bencode, peer-wire, mainline DHT, uTP, trackers - tracker/DHT/uTP traffic rides inside tunnels in anonymous mode
- Kill switch, anti-SSRF, exit-node policy enforcement
- Flutter UI: downloads, decentralized search, settings, diagnostics, logs

### Known limits

- Anonymous mode is alpha-grade: not equivalent to Tor against a global passive adversary
- Windows x64 only for now - other platforms need a source build
- API is loopback-only by default
'@

$body = @{
  tag_name   = 'v0.3.1-alpha'
  name       = 'v0.3.1-alpha - Windows x64'
  body       = $notes
  draft      = $false
  prerelease = $true
} | ConvertTo-Json

$rel = Invoke-RestMethod -Method Post -Uri "https://api.github.com/repos/$repo/releases" -Headers $h -Body $body -ContentType 'application/json'
Write-Output "release: $($rel.html_url)"

$zip = 'D:\Projet\Tribler-Rust-Torrent\dist\OnionBit-0.3.1-alpha-windows-x64.zip'
$up = ($rel.upload_url -replace '\{\?.*\}', '') + '?name=OnionBit-0.3.1-alpha-windows-x64.zip'
Invoke-RestMethod -Method Post -Uri $up -Headers ($h + @{ 'Content-Type' = 'application/zip' }) -InFile $zip | Out-Null
Write-Output 'asset uploade'
