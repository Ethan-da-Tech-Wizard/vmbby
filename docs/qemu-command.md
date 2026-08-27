# QEMU Command Breakdown

Use this command to print the current run command:

```sh
vmbby command
```

The command is generated from `~/.config/vmbby/config`.

## Main Arguments

`-name vmbby-ubuntu`

Names the VM process and window.

`-machine q35,accel=kvm`

Uses a modern PC chipset model and KVM acceleration. If `/dev/kvm` is missing,
vmbby falls back to `q35,accel=tcg`, which is slower.

`-cpu host`

Exposes a CPU model close to the host CPU to the guest. This is useful with KVM.
If KVM is unavailable, vmbby uses `-cpu max` with QEMU's slower TCG fallback.

`-m 6G`

Gives the guest a 6 GB RAM budget.

`-smp 4`

Gives the guest four virtual CPUs.

`-drive file=...,if=virtio,format=qcow2`

Attaches the Ubuntu virtual disk through a paravirtualized VirtIO disk device.

`-netdev user,id=net0`

Creates simple user-mode NAT networking.

`-device virtio-net-pci,netdev=net0`

Adds a VirtIO network card connected to that NAT backend.

`-display gtk,gl=off`

Opens a GTK display window and disables OpenGL acceleration for the display path.
This keeps VRAM pressure lower for v1.

`-device virtio-vga`

Adds a virtual GPU/display device.

`-device qemu-xhci`

Adds a USB controller.

`-device usb-tablet`

Gives the guest absolute pointer input so the mouse behaves better in a window.

`-qmp unix:...,server=on,wait=off`

Creates a QEMU Machine Protocol socket. vmbby uses this to send management
commands like clean shutdown.

`-pidfile ...`

Writes the QEMU process id so status and debugging are easier.

## Installer Arguments

`vmbby install /path/to/ubuntu.iso` adds:

`-cdrom /path/to/ubuntu.iso`

Attaches the Ubuntu installer ISO.

`-boot d`

Asks QEMU to boot from the virtual CD/DVD device first.
