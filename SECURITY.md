# Security Policy

## Supported Versions

TuwaiqOS is currently in active development.

| Version / Branch | Supported |
| ---------------- | --------- |
| `main` (latest)  | ✅ |
| Older commits and releases | ❌ |

## Reporting a Vulnerability

Please do **not** report security vulnerabilities through public GitHub Issues.

Use GitHub Private Vulnerability Reporting:

https://github.com/italamrii/TuwaiqOS/security/advisories/new

Include:

- A clear description of the issue
- Affected component or commit
- Steps to reproduce
- Expected impact
- Logs, screenshots, or a proof of concept where relevant

We aim to acknowledge valid reports within 7 days.

Please allow reasonable time for investigation and a fix before public disclosure.

## Scope

Security reports are welcome for the kernel, memory management, interrupts,
scheduler, filesystem/VFS, userspace ABI, process isolation, drivers, build
pipeline, release artifacts, and Tuwaiq AI provider/permission boundaries.

Tuwaiq AI components are userspace components. A model or provider must never
receive an undocumented privileged path, unrestricted kernel authority, or a
permission bypass; reports of such a path are in scope even during preview
development.
