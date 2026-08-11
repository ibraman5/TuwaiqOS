# Third-party components and licenses (TuwaiqOS Product D0)

This file records upstream components expected in the TuwaiqOS Product image.
It is an attribution index, not legal advice. Full texts ship with packages in
the image (`/usr/share/doc/*/copyright` on Debian/Ubuntu-style systems).

| Component | Role | Typical license(s) | Notes |
|-----------|------|--------------------|-------|
| Linux kernel | Hardware kernel | GPL-2.0-only (with syscall exception) | Not authored by TuwaiqOS |
| Ubuntu 24.04 LTS | Base OS / userspace | Mix (GPL/LGPL/MIT/BSD/Apache-2.0, …) | Canonical/Ubuntu packaging |
| systemd | Init / services | LGPL-2.1-or-later (primarily) | |
| GNU libc / related | C library toolchain | LGPL-2.1 / GPL | |
| KDE Plasma | Desktop shell | GPL-2.0-or-later / LGPL-2.0-or-later (components vary) | Not a KDE fork |
| KWin (Wayland) | Compositor | GPL-2.0-or-later | |
| SDDM | Display manager | GPL-2.0-or-later | |
| Wayland / libwayland | Display protocol | MIT | |
| Mesa | Graphics stack | MIT (primarily) | |
| PipeWire / WirePlumber | Audio/session | MIT / LGPL (components vary) | |
| NetworkManager | Networking | GPL-2.0-or-later | |
| Konsole | Terminal | GPL-2.0-or-later | |
| Dolphin | File manager | GPL-2.0-or-later | |
| Noto / Liberation fonts (if shipped) | Fonts | OFL-1.1 / Apache-2.0 (varies by family) | Confirm per package |
| Breeze / Plasma icons | Icon themes | LGPL-3.0-or-later (typical) | Branding overlays are separate |
| Tuwaiq branding assets | Product identity | See `product/branding/LICENSE` | TuwaiqOS project assets |

## Rules

1. Do not remove upstream copyright notices.
2. Do not claim Linux, KDE, Wayland, or Ubuntu as Tuwaiq inventions.
3. When adding packages, update this table in the same change.
4. Generated ISO/qcow2 images are **not** committed to Git; license texts travel
   inside the image filesystem.

## Verification

On a built rootfs/image:

```bash
# Example spot checks
dpkg -l | grep -E 'plasma|sddm|wayland|linux-image'
ls /usr/share/doc/*/copyright | head
```
