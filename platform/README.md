# Tuwaiq Platform (scaffold)

Stable product APIs live here. **D0 does not implement the full API.**

## Boundary

```
Applications
      ↓
Tuwaiq Platform API
      ↓
Linux services / desktop APIs   (product today)
```

Future:

```
Applications
      ↓
Tuwaiq Platform API
      ↓
Tuwaiq Core
```

## Planned service surfaces (Rust, future)

| Surface | Intent | D0 status |
|---------|--------|-----------|
| `launch` | Application launch | Documented only |
| `notify` | Notifications | Documented only |
| `fs` | File operations | Documented only |
| `permissions` | Capability/permission checks | Documented only |
| `sysinfo` | System information | Documented only |
| `ai` | AI requests (mediated) | Documented only |
| `system` | System actions (shutdown, …) | Documented only |

Do not expose stubs that pretend unsupported behavior works.

See `platform/services/` for placeholder module layout.
