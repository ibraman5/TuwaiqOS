# Panel / Dock

Suggested branch: `gui/panel-<github-user>`

## What exists today

A single bottom panel defined in
`product/desktop/plasma/plasma-org.kde.plasma.desktop-appletsrc`
(`[Containments][2]`, `plugin=org.kde.panel`, `location=4`):

| Applet | Plugin |
| --- | --- |
| Launcher | `org.kde.plasma.kickoff` |
| Task manager | `org.kde.plasma.icontasks` |
| Spacer | `org.kde.plasma.marginsseparator` |
| System tray | `org.kde.plasma.systemtray` |
| Clock | digital clock |

Tray items currently include network management, volume, battery, notifications,
clipboard, and keyboard layout.

Note that the shipped containments are marked `immutability=1`. If you are experimenting
interactively, you will need to relax that locally; do not ship an unlocked layout without
saying so explicitly in your PR.

## What is open

- launcher placement and panel composition
- running-application presentation
- workspace / activity awareness
- system tray behavior and overflow
- clock presentation
- responsive sizing across resolutions
- hover and focus behavior

## Constraints

- The panel is the primary navigation surface; regressions here are highly visible.
  Keep it usable at all times.
- Do not bypass NetworkManager for the network indicator; display state only.
- Panel geometry must not assume LTR ordering.
- Check both Dark and Light, and confirm text remains readable at small panel heights.
- Report idle CPU and RAM if you add any persistent panel process.
