# Proposal

## Why

Agent cells (H2 onward) need memory the kernel can hand out, address spaces
it can build, and a lower half of the address space that is free for cell
code. Today the kernel runs identity-mapped at 1 MiB on a fixed 1 GiB boot
mapping, has no allocator, and any CPU fault ends in a silent triple fault.
This is roadmap milestone **H1**.

## What Changes

- Discover usable RAM from the memory map the hypervisor passes at boot and
  manage it in 4 KiB frames.
- Move the kernel to the higher half of the address space, map all RAM
  through a direct map, and leave the lower half unmapped for future cells.
- Enforce W^X on kernel memory: code is never writable, data is never
  executable.
- Provide page-table operations (map, unmap, translate) usable for any
  address space, not only the kernel's.
- Provide a kernel heap so kernel code can use dynamic allocation.
- Report every CPU exception (with fault details) on the serial console and
  fail the VM, including stack overflows, instead of resetting silently.
- Print a memory summary during boot.

### Out of scope

Address spaces for cells and user mode (H2), copy-on-write and frame
reference counts (H5), RAM above 4 GiB, NUMA, huge-page allocation for
callers, interrupts from devices, timers, multicore.

## Capabilities

### New Capabilities

- `physical-memory`: how the kernel discovers, reserves, allocates and frees
  physical memory, and how it reports it.
- `virtual-memory`: the kernel's address-space layout, memory protections and
  page-table operations.
- `kernel-heap`: dynamic memory allocation for kernel code.
- `cpu-exceptions`: how CPU exceptions are reported and handled.

### Modified Capabilities

None. (The boot check in `kernel-testing` gains a memory line through the
`physical-memory` spec; no existing requirement changes.)

## Impact

- `rhizo-hal` gains physical-memory and TLB interfaces with host doubles.
- `rhizo-core` gains memory-map parsing, frame allocator, page tables, heap
  allocator and exception reports, all host-tested.
- Kernel boot trampoline, linker script and boot sequence change (higher-half
  kernel); new CPU setup (GDT, TSS, IDT).
- New host dev-dependency: `proptest` (MIT OR Apache-2.0).
- `cargo xtask test` gains two self-checks (page fault, stack overflow).
