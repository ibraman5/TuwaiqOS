# Tuwaiq AI (product)

Architecture (product track):

```
Tuwaiq AI
   ↓
isolated userspace process/service
   ↓
Tuwaiq Platform
   ↓
permission/policy layer
   ↓
system
```

## Rules

- Not embedded in the Linux kernel.
- No unrestricted root by default.
- AI failure must not take down the OS.
- Linux isolation ≠ Tuwaiq Core research isolation claims.

**D0:** documentation only — no AI implementation.
