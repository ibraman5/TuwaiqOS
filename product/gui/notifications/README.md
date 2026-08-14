# Notifications

Suggested branch: `gui/notifications-<github-user>`

## What exists today

Notifications are the stock Plasma applet, enabled as a tray item in
`product/desktop/plasma/plasma-org.kde.plasma.desktop-appletsrc`
(`org.kde.plasma.notifications`, listed in both `extraItems` and `shownItems`).

There is no Tuwaiq-specific notification styling or behavior yet.

## What is open

- notification popup appearance in the Tuwaiq visual language
- grouping, stacking, and overflow
- history and do-not-disturb presentation
- urgency levels and how they are expressed visually
- placement and animation

## Constraints

- Notifications must never become an execution surface. Do not add actions that run
  arbitrary commands supplied by a notification payload.
- Respect urgency: critical system messages must remain visible and legible.
- Popups must not obscure the panel or trap focus.
- Animations should be brief and cheap; no permanent high-frequency timers.
- Popup anchoring must be mirrorable for RTL.
- Verify contrast in both Dark and Light.
