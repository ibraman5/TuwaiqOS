# Quick Settings

Suggested branch: `gui/quick-settings-<github-user>`

## What exists today

There is no dedicated quick-settings surface. The equivalent functionality is currently
spread across individual system tray applets configured in
`product/desktop/plasma/plasma-org.kde.plasma.desktop-appletsrc`
(`[Containments][2][Applets][6]`): network management, volume, battery, notifications,
clipboard, and keyboard layout.

## What is open

Prototype a single coherent quick-settings surface covering:

- network status
- volume
- brightness
- Dark / Light theme toggle
- power actions
- general system status

## Constraints

This area carries the highest risk of crossing the privilege boundary. Read carefully:

- **Do not implement direct privileged hardware control.** Use existing supported system
  interfaces only.
- Networking stays under NetworkManager. Read and display its state; never write network
  configuration directly, and never shell out to reconfigure interfaces.
- Do not disable or bypass firewall or security controls.
- No plaintext credentials, and no unauthenticated IPC endpoints.
- Theme switching should use the supported Plasma 5.27 mechanisms
  (`plasma-apply-colorscheme`, `plasma-apply-lookandfeel`) rather than rewriting
  configuration files behind Plasma's back.

Design for RTL from the start: a settings panel full of left-aligned rows and
left-anchored popups is expensive to mirror later.
