# TuwaiqOS Product Architecture

This document describes the **product** track. It does not redefine Tuwaiq Core.

## Two tracks

| Track | What it is | What it is not |
|-------|------------|----------------|
| **Tuwaiq Core** | Original research kernel and related research (isolation, virtualization, AI containment proofs) | Not the current product hardware OS |
| **TuwaiqOS Product** | Daily-driver oriented desktop OS using a Linux LTS foundation with Tuwaiq identity and Platform APIs | Not a claim that Linux was built by TuwaiqOS |

```
Tuwaiq Core                 TuwaiqOS Product
─────────────               ────────────────
Research Kernel             Tuwaiq Desktop (KDE/Wayland)
Phase H / Phase 8 research  Tuwaiq Platform APIs
Preserved independently     Tuwaiq Applications
                            Tuwaiq AI (isolated userspace)
                            Linux LTS → Hardware / Drivers
```

## Why the split exists

1. **Hardware compatibility now** — Linux provides mature drivers.
2. **Usable product sooner** — a branded desktop can ship without waiting for
   full Core driver coverage.
3. **Preserve research** — Core continues without being rewritten into a
   product kernel fork.
4. **Future portability** — Platform APIs reduce Linux-specific coupling so
   selected components may later run on Core.

## Layers

### Tuwaiq Desktop

KDE Plasma + Wayland, branded as TuwaiqOS (themes, wallpaper, naming). We
configure and package; we do **not** fork KDE.

### Tuwaiq Platform

Stable product APIs for launch, notifications, files, permissions, system
info, AI requests, and system actions. D0 scaffolds the boundary only.

### Tuwaiq Applications

Prefer Platform APIs over direct Linux-only interfaces where practical.

### Tuwaiq AI

Isolated userspace service. Not in-kernel. No unrestricted root. Failure of AI
must not mean OS failure. Linux isolation ≠ Core Ring-3 proofs (kept explicit).

### Linux LTS foundation

Selected for D0: **Ubuntu 24.04 LTS**. See
[`docs/adr/0001-product-linux-foundation.md`](adr/0001-product-linux-foundation.md).

## Experience profiles (future)

| Profile | Role |
|---------|------|
| Performance | Richer effects/services |
| **Balanced** | D0 default |
| Lightweight | Reduced animations/background work |

Profiles will later gate animations, effects, background services, memory
budget, and visual complexity. D0 documents hooks only.

## Attribution

Upstream projects (Linux, Ubuntu, KDE Plasma, Wayland, fonts, icons) retain
their licenses and copyrights. See `docs/product/THIRD_PARTY.md`.
TuwaiqOS does **not** claim authorship of those projects.
