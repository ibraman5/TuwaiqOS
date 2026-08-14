# Dark / Light Visual System

Suggested branch: `gui/visual-system-<github-user>`

## What exists today

Two packaged color schemes and two look-and-feel packages:

| What | Path |
| --- | --- |
| Tuwaiq Dark colors | `product/branding/color-schemes/TuwaiqDark.colors` |
| Tuwaiq Light colors | `product/branding/color-schemes/TuwaiqLight.colors` |
| Dark look-and-feel | `product/branding/plasma/look-and-feel/org.tuwaiqos.desktop/` |
| Light look-and-feel | `product/branding/plasma/look-and-feel/org.tuwaiqos.light.desktop/` |
| Global defaults (Dark) | `product/desktop/plasma/kdeglobals` |
| Global defaults (Light) | `product/desktop/plasma/kdeglobals-light` |
| Window decoration / KWin | `product/desktop/plasma/kwinrc` |
| Konsole colors | `product/desktop/konsole/Tuwaiq.colorscheme` |

Both look-and-feel packages currently use the Breeze widget style and the default Plasma
desktop theme, with identity carried by color, wallpaper, and the Tuwaiq mark.

## Current tokens

Taken from the packaged color schemes:

| Token | Dark | Light |
| --- | --- | --- |
| Copper accent | `200,120,58` | `200,120,58` |
| Teal secondary | `61,107,111` | `42,74,78` |
| Window foreground | `245,242,235` | `20,20,22` |
| Active window background | `18,18,20` | `245,242,235` |

Copper is the constant across both themes; treat it as the identity anchor.

## What is open

- unify and refine Dark and Light
- window appearance and decoration
- typography principles
- spacing scale and corner-radius philosophy
- elevation and shadow behavior
- motion and transitions
- a documented token set that other components can reference

## Constraints

- Every change must be evaluated in **both** themes. A change that looks correct only in
  Dark is incomplete.
- Preserve copper as the primary accent and keep the teal secondary restrained.
- Maintain readable contrast; accessibility is a requirement, not a later pass.
- Theme changes must apply through supported Plasma 5.27 mechanisms and should survive an
  in-session Light → Dark switch without requiring a session restart.
- Motion should be subtle and cheap. Avoid animations that run continuously.
