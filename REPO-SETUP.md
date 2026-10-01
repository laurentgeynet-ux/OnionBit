# GitHub repository setup — checklist

Everything in this folder is ready to push to
`https://github.com/laurentgeynet-ux/OnionBit`.

## Repo settings (web UI)

- **Description** (About):
  `Anonymous BitTorrent client, native in Rust — a Tribler port with onion-routed circuits`
- **Website**: leave empty for now
- **Topics**:
  `bittorrent` `rust` `anonymity` `p2p` `tribler` `onion-routing`
  `torrent` `flutter` `privacy` `ipv8`
- **Social preview**: Settings → General → Social preview → upload
  `assets/social-preview-1280x640.png`
- **Features**: enable Issues, Discussions (optional), disable Wiki until needed
- **Security tab**: private vulnerability reporting is enabled by default on
  public repos — SECURITY.md points to it

## First push

```bash
cd <path-to>/OnionBit
git init -b main
git remote add origin https://github.com/laurentgeynet-ux/OnionBit.git
git add -A
git commit -m "Initial commit: project docs, branding, community files"
git push -u origin main
```

When the code migrates: keep this repo, push the source tree on top — the docs
and `.github/` templates stay untouched.

## Nice-to-have later

- `docs/` pages: architecture overview, threat model, Tribler mapping tables
  (source material already exists in the dev repo under `docs/`)
- CI: `verify_all.ps1` equivalent as GitHub Actions (fmt / clippy / test / flutter test)
- Release workflow using `scripts/build_release.ps1` → `dist/<target>/`
- `.github/FUNDING.yml` if you ever want sponsorship links
- Branch protection on `main` once the code lands (require PR + CI green)
