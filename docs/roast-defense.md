# Roast Defense

## Honest Claim

I built a custom VM manager for my Ubuntu school environment on Arch. It uses
QEMU/KVM underneath. My project owns the workflow around it: configuration, disk
creation, process launch, lifecycle control, logs, desktop integration, visual
identity, and technical explanation.

## What I Did Not Claim

I did not claim I wrote a full hypervisor from scratch.

I did not claim I replaced QEMU.

I did not claim I replaced KVM.

I did not claim I wrote Ubuntu.

## Why QEMU/KVM Is the Correct Boundary

Booting Ubuntu requires a complete virtual computer: CPU virtualization, memory,
timers, interrupts, firmware behavior, block storage, networking, display, USB
input, and enough device compatibility for Ubuntu's kernel and installer.

QEMU already implements the virtual device model. KVM already exposes safe
hardware virtualization through Linux. Reusing those layers is an engineering
decision, not a weakness.

The project is still useful because most people use a VM app as a black box.
vmbby turns the VM workflow into code I control and can explain.

## Good Answer Under Pressure

If someone asks "did you build VirtualBox?" the answer is:

No. I built a custom VMBox-style manager. The virtualization backend is QEMU/KVM.
My code handles the configuration model, dynamic disk creation, launch command,
process lifecycle, QMP shutdown path, host checks, Wayland workflow, GUI shell,
and explanation layer.

## Why This Is Better Than Dual Boot for School

Dual boot would make me reboot out of Arch and hand the physical laptop to
Ubuntu. That increases friction and risk.

With vmbby, Arch remains the stable host. Ubuntu runs in a controlled VM window
with fixed RAM, CPU, disk, and network settings.

## Why This Is Better Than a Container for This Requirement

A container would use the Arch kernel. If the class says Ubuntu, a real Ubuntu VM
is easier to defend because it boots Ubuntu's own kernel and userspace.
