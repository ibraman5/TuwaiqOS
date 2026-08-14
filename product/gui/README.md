# TuwaiqOS GUI Development Baseline

This is the **GUI development baseline** for TuwaiqOS. It exists so that multiple
contributors can develop the desktop experience independently and safely, through
GitHub Pull Requests, without destabilising the booting product image.

Baseline branch: `product/gui-foundation`
Baseline commit: `c1c1d99` (latest clean Product/Desktop commit)

This directory is **documentation and contributor guidance**. The GUI implementation
files already exist elsewhere in the repository and were deliberately **not moved**,
because the D0/D1 build and apply scripts depend on their current locations. See
[Where the GUI actually lives](#where-the-gui-actually-lives).

## Current foundation

| Layer | State |
| --- | --- |
| Product userspace | Ubuntu 24.04 LTS |
| Desktop | KDE Plasma 5.27 |
| Session | Plasma **X11** via SDDM — this is the qualified session |
| Compositor | `kwin_x11` |
| Shell | `plasmashell` |
| QEMU display | `virtio-vga` |

**Wayland is not qualified.** Some older product documentation mentions Wayland as the
target; treat that as a future goal, not a supported configuration. Do not submit work
that assumes a Wayland session.

Packaged and present in this baseline: Tuwaiq Dark, Tuwaiq Light, the Tuwaiq launcher
mark, panel and system-tray layout, wallpaper, SDDM configuration, and a Konsole profile.

## How to contribute

1. Fork `https://github.com/italamrii/TuwaiqOS` on GitHub.
2. Clone your fork and add the upstream remote:

```bash
git clone https://github.com/<your-user>/TuwaiqOS.git
cd TuwaiqOS
git remote add upstream https://github.com/italamrii/TuwaiqOS.git
git fetch upstream
```

3. Branch from `product/gui-foundation` — never from `main`:

```bash
git checkout -b gui/<component>-<your-user> upstream/product/gui-foundation
```

4. Make one focused GUI change, test it, push to your fork, and open a Pull Request
   against `product/gui-foundation`.

Never commit directly to `main`, `product/desktop-foundation`,
`product/d1-desktop-identity-connectivity`, or `product/gui-foundation`.

## Scope: GUI only

Work on appearance, layout, and interaction. Do **not** modify:

- kernel code
- GRUB or any boot path
- disk layout or storage
- networking or NetworkManager configuration
- Phase H (virtualization), Phase 8 (IPC/capabilities), Phase 7B (ESXi/NVMe)

NetworkManager remains the sole networking authority. GUI components may **display**
network state; they must never reconfigure it directly.

The GUI is a presentation and interaction layer, not a privilege boundary. Do not add
components that run arbitrary root commands, edit protected system files directly, store
plaintext credentials, or open network listeners. Privileged actions must go through an
existing supported system interface.

## Where the GUI actually lives

These are the real implementation paths. Edit these files; this `product/gui/` tree only
documents them.

### Identity and themes

| What | Path |
| --- | --- |
| Dark color scheme | `product/branding/color-schemes/TuwaiqDark.colors` |
| Light color scheme | `product/branding/color-schemes/TuwaiqLight.colors` |
| Dark look-and-feel package | `product/branding/plasma/look-and-feel/org.tuwaiqos.desktop/` |
| Light look-and-feel package | `product/branding/plasma/look-and-feel/org.tuwaiqos.light.desktop/` |

### Assets

| What | Path |
| --- | --- |
| Tuwaiq mark (launcher icon) | `product/branding/icons/tuwaiq-mark.svg` |
| Tuwaiq wordmark | `product/branding/icons/tuwaiq-wordmark.svg` |
| Wallpaper package | `product/branding/wallpapers/TuwaiqOS/` |

### Plasma shell configuration

| What | Path |
| --- | --- |
| Panel, launcher, tray, wallpaper layout | `product/desktop/plasma/plasma-org.kde.plasma.desktop-appletsrc` |
| Global defaults (Dark) | `product/desktop/plasma/kdeglobals` |
| Global defaults (Light) | `product/desktop/plasma/kdeglobals-light` |
| Window manager / decoration | `product/desktop/plasma/kwinrc` |
| Lock screen | `product/desktop/plasma/kscreenlockerrc` |

### Login and terminal

| What | Path |
| --- | --- |
| SDDM configuration (`DisplayServer=x11`) | `product/desktop/sddm/tuwaiqos.conf` |
| Breeze greeter overrides | `product/desktop/sddm/breeze-theme.conf.user` |
| Custom SDDM theme (not selected by default) | `product/desktop/sddm/theme/` |
| Konsole profile | `product/desktop/konsole/Tuwaiq.profile` |
| Konsole colors | `product/desktop/konsole/Tuwaiq.colorscheme` |
| Konsole defaults | `product/desktop/konsole/konsolerc` |

### Apply script and packages

| What | Path |
| --- | --- |
| Installs all of the above into a root filesystem | `product/scripts/apply-branding.sh` |
| Desktop package set | `product/packages/d1-ubuntu2404.list` |

`apply-branding.sh` is the contract between this source tree and the running system. If
you add a new asset or config file, add it there too, or it will not reach the image.

## Contributor areas

Each area has its own brief, including what already exists and what is open:

- [Launcher](launcher/README.md)
- [Panel / Dock](panel/README.md)
- [Quick Settings](quick-settings/README.md)
- [Notifications](notifications/README.md)
- [Dark/Light visual system](visual-system/README.md)
- [Icons and assets](assets/README.md)

## Visual direction

TuwaiqOS should read as premium, geometric, modern, minimal, and technical, with a Saudi
identity expressed through restraint rather than national decoration.

- Charcoal / near-black surfaces
- Warm off-white foreground
- Copper / orange as the primary accent (`200,120,58`)
- Petroleum / teal as a restrained secondary accent (`61,107,111`)
- Cinematic Tuwaiq escarpment imagery
- Geometric Tuwaiq mark and wordmark
- **No falcon branding**

The goal is a recognisably Tuwaiq desktop, not "KDE with different colors". At the same
time, do not break familiar usability patterns for the sake of novelty.

Additional expectations:

- Maintain readable contrast in both Dark and Light; every change must be checked in both.
- Do not hardcode left-to-right layout. Arabic and RTL support is a future requirement,
  and layouts that assume LTR will have to be rewritten.
- Keep contributions lightweight. Avoid polling loops, permanent high-frequency timers,
  background daemons, and heavy dependencies for small visual features. If your change
  adds a persistent process, report its idle CPU, RAM, and startup impact in the PR.

## Testing your change

Most of this baseline is Plasma configuration and assets, so the fastest loop is a KDE
Plasma 5.27 environment where you can apply a file and restart only the affected component:

```bash
# color scheme / look-and-feel (supported Plasma 5.27 tools)
plasma-apply-colorscheme TuwaiqDark
plasma-apply-lookandfeel -a org.tuwaiqos.desktop

# restart only the shell after editing panel/launcher configuration
kquitapp5 plasmashell && kstart5 plasmashell
```

Then capture a screenshot for your PR.

Full integration is verified by applying the source tree into a root filesystem with
`product/scripts/apply-branding.sh` and booting the product image in QEMU. That path is
slower and currently requires the image build tooling in `product/build/`. Honest caveat:
there is no lightweight "preview the whole TuwaiqOS desktop" harness yet. Faster iteration
tooling is a worthwhile future contribution.

Do not commit build artifacts: no `.qcow2`, `.raw`, or ISO images, no Docker volumes, no
package caches, no large build logs.

## Pull Request checklist

Copy this into your PR description:

```markdown
- Component changed:
- Design goal:
- Screenshots (Dark and Light):
- Video/GIF (if motion matters):
- Files changed:
- Dependencies added:
- RAM/CPU impact (if a persistent process was added):
- Accessibility considerations:
- RTL considerations:
- Testing performed:
- Known limitations:

- [ ] I did not modify kernel code
- [ ] I did not modify GRUB/boot
- [ ] I did not modify networking architecture
- [ ] I did not modify Phase H
- [ ] I did not modify Phase 8
- [ ] I did not modify Phase 7B
- [ ] I did not add secrets
- [ ] I did not add unnecessary root privileges
- [ ] I tested the GUI change
- [ ] I included visual evidence
```
