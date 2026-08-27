# vmbby Architecture

## Stack

```text
Arch Linux host
  Plasma Wayland or Hyprland Wayland
    vmbby Rust manager
      QEMU process
        KVM through /dev/kvm
          Ubuntu guest kernel and userspace
```

## Host and Guest

The host is the operating system that owns the physical machine. In this project
the host is Arch Linux.

The guest is the operating system running inside a virtual machine. In this
project the guest is Ubuntu 24.04 LTS.

Arch owns the physical CPU, RAM, disk, GPU, keyboard, display server, and process
table. Ubuntu sees virtual versions of those resources.

## QEMU

QEMU is the virtual machine process. It gives Ubuntu a virtual PC:

- a machine type (`q35`)
- virtual CPUs
- guest RAM
- a virtual disk controller
- a virtual network card
- a display device
- USB tablet input
- firmware/boot behavior
- a QMP management socket

Without QEMU, using KVM directly would only give a low-level CPU virtualization
interface. Ubuntu also needs devices, interrupts, timers, firmware, storage, and
networking. That is why QEMU is the right lower layer for a school-ready Ubuntu
VM.

## KVM

KVM is the Linux kernel's virtualization interface. It is exposed through:

```text
/dev/kvm
```

With KVM, guest instructions can run on the real Intel CPU using hardware
virtualization. Privileged guest behavior traps back to the host kernel. That is
how Ubuntu can believe it owns a machine without actually taking over Arch.

If `/dev/kvm` is missing, QEMU can sometimes fall back to pure emulation, but it
will be much slower. `vmbby doctor` calls that out.

## Disk

The Ubuntu disk is a `qcow2` image:

```text
~/.local/share/vmbby/ubuntu.qcow2
```

`qcow2` is dynamic. A `60G` virtual disk does not immediately consume 60 GB on
the host. It grows as the guest writes data.

Ubuntu sees this as a hard drive. Arch sees it as a regular file.

## Networking

vmbby uses QEMU user-mode NAT:

```text
-netdev user,id=net0
-device virtio-net-pci,netdev=net0
```

This gives Ubuntu outbound network access without requiring a bridge, TAP
device, NetworkManager changes, or root-only host networking setup.

## Display on Plasma and Hyprland

The Ubuntu VM window is a QEMU GTK window. Plasma and Hyprland are only the host
compositors displaying that window.

This means the same VM manager works in:

- KDE Plasma Wayland
- Hyprland Wayland

The compositor does not create the VM. It only displays and manages the window.

## Not Dual Boot

Dual boot means rebooting the physical machine and choosing Arch or Ubuntu.
vmbby does not do that. Arch stays running and Ubuntu runs as a process.

## Not a Container

A container shares the host kernel. vmbby boots an Ubuntu kernel inside a virtual
machine. This is closer to what a class means when it says work must be done on
Ubuntu.
