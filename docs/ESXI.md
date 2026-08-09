# TuwaiqOS ESXi Phase 7B Artifact

This directory contains a generated BIOS/NVMe virtual machine using virtual
hardware version `{{VIRTUAL_HW_VERSION}}`. The VMDK and VMX are generated
together; do not edit either descriptor manually.

## Import

1. Upload `TuwaiqOS-ESXi.vmdk` and `TuwaiqOS-ESXi.vmx` to one datastore folder.
2. Verify the files against `SHA256SUMS.txt`.
3. Register the VMX, confirm BIOS firmware, one vCPU, 512 MiB RAM, one NVMe
   disk, and a file-backed COM1 serial port.
4. Keep networking disabled for this storage/boot milestone.
5. Power on and preserve `TuwaiqOS-ESXi-COM1.log` before changing VM settings.

The guest prints allocation-free `BOOT: stage=...` breadcrumbs to COM1 before
framebuffer selection. A framebuffer rejection falls back to VGA text and then
serial-only diagnostics instead of hiding the failure behind a black screen.

## Validation status

Artifact conversion and QEMU NVMe verification do not prove ESXi compatibility.
ESXi validation remains **pending** until an external tester returns:

- ESXi version and build;
- VM compatibility/virtual-hardware version;
- complete VM settings;
- a screenshot of the final display state; and
- the complete COM1 serial log.

Do not mark Phase 7B or physical qualification complete without that evidence.

