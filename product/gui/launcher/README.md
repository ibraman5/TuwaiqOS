# Launcher

Suggested branch: `gui/launcher-<github-user>`

## What exists today

The launcher is stock Plasma Kickoff with Tuwaiq branding applied through configuration:

- Applet and icon: `product/desktop/plasma/plasma-org.kde.plasma.desktop-appletsrc`
  (`[Containments][2][Applets][3]`, `plugin=org.kde.plasma.kickoff`,
  `icon=/usr/share/tuwaiqos/icons/tuwaiq-mark.svg`)
- Icon source: `product/branding/icons/tuwaiq-mark.svg`
- Default system favorites: System Settings, Konsole, Dolphin

Kickoff can be opened programmatically for testing through the Plasma shell D-Bus API
(`org.kde.plasmashell` → `evaluateScript`, setting the applet's `expanded` property).
Prefer that over keyboard automation, which has proven unreliable in QEMU.

## What is open

Design and prototype a distinctive Tuwaiq application launcher:

- application search and result ranking
- categories
- favorites
- recent applications
- system actions (presented in the UI; see the privilege note below)
- Tuwaiq visual identity applied to the menu surface itself, not just the button
- full keyboard navigation

## Constraints

- Do not implement privileged system operations. Power and session actions must go
  through existing supported Plasma/systemd interfaces, never through custom root helpers.
- Keyboard navigation is a requirement, not an enhancement.
- Do not hardcode LTR layout; the menu will need to mirror for Arabic.
- Verify appearance in both Tuwaiq Dark and Tuwaiq Light.
