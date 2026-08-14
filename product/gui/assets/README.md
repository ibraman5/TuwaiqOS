# Icons and Assets

Suggested branch: `gui/assets-<github-user>`

## What exists today

| Asset | Path | Used by |
| --- | --- | --- |
| Tuwaiq mark | `product/branding/icons/tuwaiq-mark.svg` | Kickoff launcher button |
| Tuwaiq wordmark | `product/branding/icons/tuwaiq-wordmark.svg` | branding surfaces |
| Wallpaper | `product/branding/wallpapers/TuwaiqOS/contents/images/1920x1080.svg` | desktop and SDDM greeter |
| Wallpaper metadata | `product/branding/wallpapers/TuwaiqOS/metadata.desktop` | Plasma wallpaper package |
| Branding license | `product/branding/LICENSE` | — |

The icon theme is currently stock Breeze (`breeze-dark` for Dark, `breeze` for Light),
set in `kdeglobals` / `kdeglobals-light`. There is no Tuwaiq icon theme yet.

Assets are installed into the image by `product/scripts/apply-branding.sh`, which places
icons under `/usr/share/tuwaiqos/icons/` and the wallpaper under `/usr/share/wallpapers/`.
**If you add an asset, add it to that script**, or it will not reach the running system.

## What is open

- a coherent Tuwaiq icon philosophy, and eventually an icon theme
- additional wallpaper resolutions and variants
- Dark/Light-aware asset variants
- refinement of the mark and wordmark at small sizes

## Constraints

- Prefer SVG so assets scale cleanly.
- The wallpaper is the cinematic Tuwaiq escarpment: charcoal and copper, geometric.
- **No falcon branding.**
- Check the launcher mark at actual panel size, not just zoomed in.
- Keep file sizes reasonable; this repository must not accumulate large binaries.
- Respect `product/branding/LICENSE` for anything under `product/branding/`.
