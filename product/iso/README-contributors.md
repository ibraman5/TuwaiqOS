# TuwaiqOS Developer ISO — Contributor Guide

Welcome. This is a **live** developer image of TuwaiqOS for GUI contributors.
It boots the real TuwaiqOS Plasma desktop so you can see and test your changes.

Because it is a live image, **changes you make inside this session are lost on
reboot.** Push your work to GitHub before shutting down.

Login user: `tuwaiq` / password: `tuwaiq` (autologin is enabled)

## 1. Repository

https://github.com/italamrii/TuwaiqOS

## 2. Base branch

All GUI work starts from:

    product/gui-foundation

Never branch from `main` for GUI work.

## 3. Fork the repository

Open the repository on GitHub and click **Fork**.

## 4. Clone your fork and create your branch

```bash
git clone https://github.com/<your-username>/TuwaiqOS.git
cd TuwaiqOS
git remote add upstream https://github.com/italamrii/TuwaiqOS.git
git fetch upstream

git checkout -b gui/<component>-<your-username> upstream/product/gui-foundation
```

Examples: `gui/launcher-yazeed`, `gui/panel-sara`, `gui/quick-settings-khalid`

## 5. Work on the GUI only

Edit only desktop appearance, layout, and interaction. Do **not** modify:

- kernel code
- GRUB or the boot path
- disk layout or storage
- networking or NetworkManager configuration
- Phase H, Phase 8, or Phase 7B

NetworkManager stays the networking authority. GUI components may display network
state, never reconfigure it. The GUI is not a privilege boundary: no arbitrary root
commands, no plaintext credentials, no unauthenticated IPC.

The GUI source lives here in the repository:

| Area | Path |
| --- | --- |
| Color schemes | `product/branding/color-schemes/` |
| Look-and-feel packages | `product/branding/plasma/look-and-feel/` |
| Icons and wallpaper | `product/branding/icons/`, `product/branding/wallpapers/` |
| Panel / launcher / tray | `product/desktop/plasma/plasma-org.kde.plasma.desktop-appletsrc` |
| Plasma globals | `product/desktop/plasma/kdeglobals`, `kdeglobals-light` |
| SDDM | `product/desktop/sddm/` |
| Konsole | `product/desktop/konsole/` |
| Contributor briefs | `product/gui/` |

A copy of the GUI source and documentation is also on this ISO at
`/usr/share/tuwaiqos/gui-source/` for offline reference.

## 6. Test your change on this live desktop

Most of the desktop is Plasma configuration, so you can apply changes without
rebuilding anything:

```bash
# themes (supported Plasma 5.27 tools)
plasma-apply-colorscheme TuwaiqDark
plasma-apply-colorscheme TuwaiqLight
plasma-apply-lookandfeel -a org.tuwaiqos.desktop

# after editing panel/launcher configuration, restart only the shell
kquitapp5 plasmashell && kstart5 plasmashell
```

Check your change in **both** Dark and Light, then take a screenshot with Spectacle
(PrtSc) for your Pull Request.

## 7. Push to your fork

```bash
git add -A
git commit -m "gui: describe your change"
git push -u origin gui/<component>-<your-username>
```

## 8. Open a Pull Request

Open the PR against `product/gui-foundation` (not `main`) and include:

- component changed, and the design goal
- screenshots in Dark and Light
- files changed, and any dependencies added
- RAM/CPU impact if you added a persistent process
- accessibility and RTL considerations
- testing performed, and known limitations

Confirm in the PR that you did not modify the kernel, GRUB/boot, networking,
Phase H, Phase 8, or Phase 7B; that you added no secrets and no unnecessary root
privileges; and that you included visual evidence.

## Never commit directly to `main`

`main`, `product/desktop-foundation`, `product/d1-desktop-identity-connectivity`,
and `product/gui-foundation` are all off limits for direct commits. Everything
arrives through a Pull Request.
