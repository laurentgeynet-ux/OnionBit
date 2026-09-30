# Branding OnionBit

Sources SVG (modifiables) et exports raster par plateforme.

## Sources

| Fichier | Usage |
| :--- | :--- |
| `icon.svg` | Glyphe seul, fond transparent (favicon, README, petites tailles) |
| `app-icon.svg` | Icône arrondie fond sombre (base des exports raster) |
| `icon-maskable.svg` | Variante plein cadre (maskable Web / zone sure) |
| `icon-android-foreground.svg` | Calque foreground des adaptive icons Android (~62 %) |
| `logo-horizontal.svg` | Logo + wordmark « OnionBit » (en-tete README, docs) |
| `github-social.svg` | Banniere 1280x640 (Social preview GitHub) |

Palette : violet oignon `#7D4698` → `#6C2EA6`, accent cyan `#1FA8B8` → `#4FD8E0`, fond `#150F1F`/`#241B33`.

## Exports

- `platforms/android/` — `mipmap-*/ic_launcher.png` (legacy),
  `ic_launcher_foreground.png` + `mipmap-anydpi-v26/ic_launcher.xml` +
  `values/ic_launcher_background.xml` (adaptive), `playstore-icon-512.png`.
  A copier sous `app/android/app/src/main/res/`.
- `platforms/linux/` — theme hicolor `*/apps/onionbit.png` + SVG scalable
  + `onionbit.desktop`. A installer dans
  `~/.local/share/icons/hicolor/` et `~/.local/share/applications/`.
- Windows : `app/windows/runner/resources/app_icon.ico` (multi-tailles).
- Web : `app/web/favicon.png`, `app/web/icons/Icon*.png`.

## Regeneration

```bash
# PNG : sharp-cli (Node)
npx -y sharp-cli -i branding/app-icon.svg -o out.png resize 512 512

# ICO : Pillow
python -c "from PIL import Image; ..."
```
