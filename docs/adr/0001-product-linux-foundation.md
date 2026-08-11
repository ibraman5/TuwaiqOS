# ADR 0001: TuwaiqOS Product uses a Linux LTS foundation

- Status: Accepted
- Date: 2026-08-11
- Milestone: Tuwaiq Desktop D0 — Product Foundation

## Context

Tuwaiq Core is the original research kernel. It remains an active research
program (memory safety, isolation, virtualization, AI containment). Building a
daily-driver desktop solely on Tuwaiq Core would require years of hardware
driver enablement before the product could be usable.

Users need a visible, bootable TuwaiqOS product sooner. The mature Linux driver
ecosystem provides that path without abandoning Core research.

## Decision

TuwaiqOS splits into two tracks that must never be confused:

### 1. Tuwaiq Core (research)

- The original Tuwaiq kernel and related research (including Phase H
  virtualization work).
- Continues as an independent research program.
- Is **not** replaced by Linux.
- Is **not** claimed to be the current product hardware kernel.

### 2. TuwaiqOS Product (desktop)

- Uses a **Linux LTS** distribution as the hardware/runtime foundation.
- Ships **Tuwaiq Desktop** (KDE Plasma + Wayland, branded as TuwaiqOS).
- Exposes **Tuwaiq Platform** APIs above the kernel boundary.
- Runs **Tuwaiq AI** as an isolated userspace service (not in-kernel).
- Hosts **Tuwaiq Applications** that prefer Platform APIs over Linux-specific
  coupling.

```
TuwaiqOS Product
        │
        ├── Tuwaiq Desktop
        ├── Tuwaiq Platform API
        ├── Tuwaiq Applications
        ├── Tuwaiq AI
        └── Linux LTS
                └── Hardware / Drivers
```

Long-term rule: applications should depend on Tuwaiq Platform APIs so selected
components can later target Tuwaiq Core without rewriting product UX.

## Selected Linux base (D0)

**Ubuntu 24.04 LTS (Noble Numbat)**

Technical reasons (not branding preference):

1. **LTS window** — supported through April 2029 (standard support).
2. **Desktop packages** — Plasma desktop and Wayland session packages are
   available and maintained for this LTS.
3. **Firmware/hardware** — broad `linux-firmware` and OEM coverage useful for a
   first product image.
4. **Build reproducibility** — official Docker/OCI images and well-documented
   live/rootfs tooling suit CI and Windows-hosted Docker builds.
5. **Security maintenance** — predictable CVE process for a product track.

Alternatives considered:

| Base | Rejected for D0 because |
|------|-------------------------|
| Debian 12 Bookworm | Excellent reproducibility, but shorter remaining desktop currency for Plasma/Wayland relative to Ubuntu 24.04 LTS horizon for this product start. |
| Fedora | Shorter release cycle; less aligned with “conservative LTS foundation.” |
| Custom kernel fork | Explicitly out of scope; we do not fork Linux. |

We do **not** create a custom package manager. We do **not** patch Linux unless
absolutely required and documented.

## Desktop foundation

- **KDE Plasma** for the desktop shell.
- **Wayland** as the primary session type.
- Product identity applied via configuration, themes, and packaging — **not** a
  KDE fork.

## Platform boundary

```
Applications → Tuwaiq Platform API → Linux services / desktop APIs
```

Future:

```
Applications → Tuwaiq Platform API → Tuwaiq Core
```

Unsupported features must not be pretended into the API surface.

## AI architecture (product)

```
Tuwaiq AI
   → isolated userspace process/service
   → Tuwaiq Platform
   → permission/policy layer
   → system
```

- AI is **not** embedded in the Linux kernel.
- AI does **not** receive unrestricted root by default.
- AI failure must not mean OS failure.
- Linux isolation mechanisms may be used; they are **not** claimed equivalent to
  Tuwaiq Core Ring-3 / research isolation proofs.

## Experience profiles (documented for later)

| Profile | Intent |
|---------|--------|
| Performance | Higher effects/animations; fuller background services |
| Balanced | Default D0 target |
| Lightweight | Reduced effects/services/memory |

D0 does not implement automatic hardware detection; profiles are documented
hooks only.

## Consequences

### Positive

- Faster path to a bootable, branded desktop.
- Research Core remains preserved and unblocked.
- Clear API seam for future Core migration.

### Negative / risks

- Dual-track complexity and communication cost.
- Must never market “Linux became Tuwaiq Core.”
- Licensing/attribution obligations for upstream components.
- Product ≠ research isolation claims.

## Non-goals for D0

- Full Platform API implementation
- Tuwaiq AI product implementation
- Custom installer / app store / Wine / TuwaiqBox
- Secure Boot / rollback updater
- H4 virtualization / Phase 9 Core work
- KDE or Linux forks

## References

- Repository tracks: `main` (Core product history), `phaseH/*` (virtualization
  research), `phase8/*` (IPC research), `product/desktop-foundation` (this track).
- Upstream: Ubuntu, Linux kernel, KDE Plasma, Wayland — retained with
  attribution; not claimed as Tuwaiq inventions.
