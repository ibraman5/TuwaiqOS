# TuwaiqOS Product (Desktop track)

Branch: `product/desktop-foundation`  
Milestone: **Tuwaiq Desktop D0 — Product Foundation**

## Quick start

```powershell
# Requires Docker Desktop running
.\scripts\build-product.ps1
```

```bash
# Linux/macOS host with Docker
./scripts/build-product.sh
```

Artifacts (gitignored):

- `product/out/tuwaiqos-d0.raw`
- `product/out/tuwaiqos-d0.qcow2` (when qemu-img is available in the builder)

Smoke:

```powershell
.\scripts\product-desktop-smoke.ps1
```

## Layout

```
product/          # image config, branding, packages, build
platform/         # Platform API boundary (scaffold)
apps/             # future applications
ai/               # future isolated AI service docs
docs/adr/         # architecture decision records
docs/product/     # product architecture + licensing
scripts/          # build-product + smoke
```

## Identity

- Name: **TuwaiqOS** (EN) / **طويق أو إس** (AR)
- Desktop: KDE Plasma + Wayland
- Base: Ubuntu 24.04 LTS (see ADR 0001)

## Safety

This track does **not** modify Tuwaiq Core research branches (Phase H, Phase 8, …).
