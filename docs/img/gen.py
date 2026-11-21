#!/usr/bin/env python3
"""Generate the README graphics (light and dark SVGs) in docs/img/ from one
benchmark run, so the charts agree with the README's generated tables:

    python3 docs/img/gen.py bench-results.json [bench-results-aarch64.json]

`bench-results.json` is what `cargo xtask bench --output` writes (CI
publishes it as the `bench-results-N` annotations); regenerate the tables
from the same file with `cargo xtask bench --from bench-results.json
--readme`. Figures use the Firecracker column; ranges span every VMM.

The optional second file is the AArch64 job's instruction counts (the
`bench-aarch64-results-N` annotations): with it, `arches.svg` shows them
next to x86_64's (pass it to `--readme` too, as a second `--from`).
"""
import json
import math
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the images
from svg import OUT, Svg, both  # noqa: E402
RESULTS = json.load(open(sys.argv[1]))
for extra in sys.argv[2:]:
    RESULTS["metrics"] += json.load(open(extra))["metrics"]


def median(name, guest="rhizo", vmm="firecracker", required=True):
    for m in RESULTS["metrics"]:
        if m["name"] == name and m["guest"] == guest and m["vmm"] == vmm:
            return m["median"]
    if required:
        raise SystemExit(f"{name}/{guest}/{vmm} is not in the results")
    return None


def spread(name):
    """(min, max) of a Rhizo metric over the VMMs."""
    v = [m["median"] for m in RESULTS["metrics"] if m["name"] == name and m["guest"] == "rhizo"]
    return min(v), max(v)


def us(v):
    """A time given in microseconds, as people write it."""
    if v >= 1000:
        return f"{v / 1000:.2f} ms"
    return f"{v:.0f} µs" if v >= 10 else f"{v:.2f} µs"


def times(fast, slow):
    """'≈ N× faster' (or slower, when `fast` is not), N to two significant
    figures; "about the same" within 10%."""
    if 1 / 1.1 < slow / fast < 1.1:
        return "about the same"
    if fast > slow:
        r = fast / slow
        r = round(r, 1 - int(math.floor(math.log10(r))))
        return f"≈ {r:,.0f}× slower" if r >= 10 else f"≈ {r:.1f}× slower"
    r = slow / fast
    r = round(r, 1 - int(math.floor(math.log10(r))))
    return f"≈ {r:,.0f}× faster" if r >= 10 else f"≈ {r:.1f}× faster"


def span(name):
    lo, hi = spread(name)
    return f"{lo:.1f}–{hi:.1f} µs" if f"{lo:.1f}" != f"{hi:.1f}" else f"{lo:.1f} µs"


RESUME = median("cell.resume_us")
FORK = median("cell.fork_us")
RUN = median("cell.run_us")
TEMPLATE = median("cell.spawn_from_template_us")
FC_RESTORE = median("snapshot.restore_ms", "linux") * 1000
RESUME_EAGER = median("cell.resume_eager_us")
RESUME_TOUCH = median("cell.resume_touch_us")
FORK_TOUCH = median("cell.fork_touch_us")


def process(name):
    """A Linux process baseline: inside the same Firecracker VM as the cells
    when measured there, else on the host. Returns (value, where)."""
    v = median(name, "linux-process", "firecracker", required=False)
    if v is not None:
        return v, "same VM"
    v = median(name, "linux-process", "host", required=False)
    return v, "host"


LINUX_CALL, CALL_AT = process("process.fork_call_us")
LINUX_TOUCH, TOUCH_AT = process("process.fork_touch_us")
LINUX_SPAWN, SPAWN_AT = process("process.spawn_us")
CRIU = median("snapshot.restore_ms", "criu", "host", required=False)
CRIU = CRIU and CRIU * 1000
GV_RUN = median("sandbox.run_ms", "gvisor", "runsc", required=False)
GV_RESTORE = median("snapshot.restore_ms", "gvisor", "runsc", required=False)
GV_RUN = GV_RUN and GV_RUN * 1000
GV_RESTORE = GV_RESTORE and GV_RESTORE * 1000
THREADS = median("cell.thread_create_us", required=False)
SCALING = median("cell.scaling_pct", required=False)
LOOPBACK = median("cell.loopback_rtt_us", required=False)
CPY_COLD = median("workload.cpython_cold_us", required=False)
CPY_WARM = median("workload.cpython_warm_us", required=False)
LX_CPY_COLD = median("workload.cpython_cold_us", "linux-process", required=False)
LX_CPY_WARM = median("workload.cpython_warm_us", "linux-process", required=False)
NP_COLD = median("workload.numpy_cold_us", required=False)
NP_WARM = median("workload.numpy_warm_us", required=False)
LX_NP_COLD = median("workload.numpy_cold_us", "linux-process", required=False)
LX_NP_WARM = median("workload.numpy_warm_us", "linux-process", required=False)
NODE_COLD = median("workload.node_cold_us", required=False)
NODE_WARM = median("workload.node_warm_us", required=False)
LX_NODE_COLD = median("workload.node_cold_us", "linux-process", required=False)
SECURITY = (OUT.parent / "security.md").exists()
PY_COLD = median("workload.python_cold_us")
PY_WARM = median("workload.python_warm_us")
IDLE_KB = median("cell.idle_bytes", vmm=None) / 1000
# ---------------------------------------------------------------- chart
def chart(t):
    def linux(label, value, at):
        return [(f"Linux {label}" + ("" if at == "same VM" else " (host)"), value, us(value),
                 False)] if value else []

    def ratio(fast, slow):
        return times(fast, slow) if slow else ""

    # Tier 1: the same operation, done by a cell and by a Linux process in the
    # same kind of VM (design D3 of `add-fair-comparisons`).
    fair = [
        ("Fork, holding 50 MiB (the fork call)", ratio(FORK, LINUX_CALL),
         [("Rhizo cell", FORK, us(FORK), True)] + linux("fork()", LINUX_CALL, CALL_AT)),
        ("Fork, then the child writes all 50 MiB", ratio(FORK_TOUCH, LINUX_TOUCH),
         [("Rhizo cell", FORK_TOUCH, us(FORK_TOUCH), True)]
         + linux("fork + write", LINUX_TOUCH, TOUCH_AT)),
        ("Run hello, from start to exit", ratio(RUN, LINUX_SPAWN),
         [("Rhizo cell", RUN, us(RUN), True)] + linux("posix_spawn", LINUX_SPAWN, SPAWN_AT)),
        ("Resume a suspended 8 MiB program", ratio(RESUME_EAGER, CRIU),
         [("Cell, lazy", RESUME, us(RESUME), True),
          ("Cell, eager", RESUME_EAGER, us(RESUME_EAGER), True),
          ("Cell, lazy + read all", RESUME_TOUCH, us(RESUME_TOUCH), True)]
         + ([("CRIU restore (host)", CRIU, us(CRIU), False)] if CRIU else [])),
    ] + ([
        ("CPython agent request, cold start", ratio(CPY_COLD, LX_CPY_COLD),
         [("Rhizo cell", CPY_COLD, us(CPY_COLD), True)]
         + linux("process", LX_CPY_COLD, "same VM")),
        ("CPython agent request, from a warm template", ratio(CPY_WARM, LX_CPY_WARM),
         [("Rhizo cell", CPY_WARM, us(CPY_WARM), True)]
         + linux("fork of warm", LX_CPY_WARM, "same VM")),
    ] if CPY_COLD and CPY_WARM else []) + ([
        ("numpy from PyPI wheels, cold start", ratio(NP_COLD, LX_NP_COLD),
         [("Rhizo cell", NP_COLD, us(NP_COLD), True)]
         + linux("process", LX_NP_COLD, "same VM")),
        ("numpy from PyPI wheels, from a warm template", ratio(NP_WARM, LX_NP_WARM),
         [("Rhizo cell", NP_WARM, us(NP_WARM), True)]
         + linux("fork of warm", LX_NP_WARM, "same VM")),
    ] if NP_COLD and NP_WARM else []) + ([
        # Node.js cannot fork itself: a Linux process can only start cold.
        ("Node.js agent request (Linux cannot fork Node.js)", ratio(NODE_WARM, LX_NODE_COLD),
         [("Cell, warm template", NODE_WARM, us(NODE_WARM), True),
          ("Cell, cold", NODE_COLD, us(NODE_COLD), False)]
         + linux("process, cold", LX_NODE_COLD, "same VM")),
    ] if NODE_COLD and NODE_WARM else []) + [
        ("Answer a Python request (MicroPython)", ratio(PY_WARM, PY_COLD),
         [("Warm template", PY_WARM, us(PY_WARM), True),
          ("Cold interpreter", PY_COLD, us(PY_COLD), False)]),
    ]
    # Tier 2: whole VMs and sandboxes. Not the same scope as a cell: shown
    # for scale, without a speed-up.
    whole = [
        ("Resume a whole environment", "",
         [("Cell, eager", RESUME_EAGER, us(RESUME_EAGER), True),
          ("Firecracker VM", FC_RESTORE, us(FC_RESTORE), False)]
         + ([("gVisor sandbox", GV_RESTORE, us(GV_RESTORE), False)] if GV_RESTORE else [])),
    ] + ([("Run hello in a new environment", "",
           [("Rhizo cell", RUN, us(RUN), True),
            ("gVisor sandbox", GV_RUN, us(GV_RUN), False)])] if GV_RUN else [])
    tiers = [("Same operation, same kind of VM: a cell against a Linux process", fair),
             ("Whole VMs and sandboxes: a larger scope, shown for scale", whole)]
    x0, x1 = 200, 676

    def X(v, longest):
        """Linear within a comparison: the longest bar spans the whole width,
        so bar lengths keep the ratios the labels state."""
        return x0 + max(3.0, v / longest * (x1 - x0))

    def gheight(g):
        return 30 + 32 * len(g[2]) + 18

    top = 104
    tier_h = 44
    bottom = top + sum(tier_h + sum(gheight(g) for g in gs) for _, gs in tiers)
    h = bottom + 78
    s = Svg(t, h, "Measured on the same machine",
            "Bar chart, lower is better, bars to scale within each comparison. "
            + " ".join(f"{name}. " + " ".join(
                f"{g[0]}: " + ", ".join(f"{n} {lab}" for n, _, lab, _ in g[2]) + "."
                for g in gs) for name, gs in tiers))
    s.text(24, 40, "Measured on the same machine", 20, weight=700)
    s.text(24, 64, "Time per operation; bars to scale within each comparison; "
           "lower is better", 14, fill=t["muted"])
    s.rect(24, 78, 14, 14, t["bar"], t["bar"], rx=3, sw=0)
    s.text(44, 90, "Rhizo", 14)
    s.rect(110, 78, 14, 14, t["other"], t["other"], rx=3, sw=0)
    s.text(130, 90, "Existing mechanism, or a cold start", 14)
    y = top
    for tier, gs in tiers:
        s.line(24, y + 14, 776, y + 14, color=t["boxline"], arrow=False, sw=1)
        s.text(24, y + 34, tier, 15, fill=t["accent"], weight=700)
        y += tier_h
        for title, rt, bars in gs:
            s.text(24, y + 22, title, 15, weight=700)
            if rt:
                color = (t["warn"] if "slower" in rt else
                         t["muted"] if rt == "about the same" else t["accent"])
                s.text(776, y + 22, rt, 15, fill=color, weight=700, anchor="end")
            by = y + 34
            longest = max(v for _, v, _, _ in bars)
            for name, v, lab, rh in bars:
                c = t["bar"] if rh else t["other"]
                s.text(x0 - 10, by + 17, name, 14, fill=t["text"] if rh else t["muted"],
                       anchor="end")
                s.rect(x0, by + 3, round(X(v, longest) - x0, 1), 20, c, c, rx=3, sw=0)
                s.add(f'<text x="{X(v, longest) + 8}" y="{by + 18}" font-size="14" fill="{t['text']}" '
                      f'font-weight="{700 if rh else 400}" stroke="{t['bg']}" '
                      f'stroke-width="5" stroke-linejoin="round" paint-order="stroke">'
                      f'{lab}</text>')
                by += 32
            y += gheight((title, rt, bars))
    cy = bottom + 20
    s.text(24, cy, "GitHub-hosted runners with KVM (shared machines, nested virtualization). "
           "Rhizo and", 14, fill=t["muted"])
    s.text(24, cy + 19, "the Linux processes run inside Firecracker; CRIU runs on the host. "
           "Eager resume restores", 14, fill=t["muted"])
    s.text(24, cy + 38, "every page first; lazy brings pages back on first touch.", 14,
           fill=t["muted"])
    return s


# ---------------------------------------------------------------- scorecard
def scorecard(t):
    s = Svg(t, 250, "Thesis targets and where they stand",
            f"Fork a 50 MB cell: target under 1 ms, measured {span('cell.fork_us')}, met. "
            f"Resume from memory: target under 100 µs, measured {span('cell.resume_us')} "
            f"with lazy restore, met ({us(RESUME_EAGER)} eager). "
            "Overhead per idle cell: target a few KB plus its "
            f"own pages, measured {IDLE_KB:.0f} KB with 1,000 idle cells live, met.")
    cards = [
        ("Fork a ~50 MB cell", "target < 1 ms", span("cell.fork_us"), "copy-on-write", True),
        ("Resume from memory", "target < 100 µs", span("cell.resume_us"),
         f"lazy; eager {us(RESUME_EAGER)}", True),
        ("Idle cell overhead", "target: a few KB", f"{IDLE_KB:.0f} KB",
         "1,000 idle cells live", True),
    ]
    cw, gap, x = 240, 16, 24
    for title, target, value, note, met in cards:
        s.rect(x, 24, cw, 202, t["cell"] if met else t["box"],
               t["cellline"] if met else t["boxline"], rx=12)
        s.text(x + 18, 56, title, 16, weight=700)
        s.text(x + 18, 80, target, 14, fill=t["muted"])
        s.text(x + 18, 134, value, 30, weight=700,
               fill=t["accent"] if met else t["text"])
        s.text(x + 18, 160, note, 14, fill=t["muted"])
        if met:
            dot(s, t, x + 27, 194, "done")
            s.text(x + 44, 199, "Met", 14, fill=t["accent"], weight=700)
        else:
            dot(s, t, x + 27, 194, "now")
            s.text(x + 44, 199, "In progress", 14, fill=t["warn"], weight=700)
        x += cw + gap
    return s


# ---------------------------------------------------------------- arches
ARCH_OPS = [
    ("cell.syscall_insns", "System call round trip"),
    ("cell.switch_insns", "Switch between two cells"),
    ("cell.run_insns", "Run hello, start to exit"),
    ("cell.fork_insns", "Fork a 50 MiB cell"),
    ("cell.resume_insns", "Resume 8 MiB, lazy"),
    ("cell.resume_eager_insns", "Resume 8 MiB, eager"),
    ("cell.pipe_rtt_insns", "Pipe round trip"),
    ("workload.warm_insns", "glibc program, warm"),
]


def arches(t):
    """Instructions per operation under QEMU -icount on both architectures,
    or None without AArch64 figures."""
    rows = [(label, median(name, vmm="qemu-microvm-icount", required=False),
             median(name, vmm="qemu-virt-icount", required=False))
            for name, label in ARCH_OPS]
    rows = [r for r in rows if r[1] and r[2]]
    if not rows:
        return None
    x0, x1, gh = 230, 650, 64
    top = 104
    h = top + gh * len(rows) + 52
    s = Svg(t, h, "Both architectures, in instructions",
            "Bar chart of retired instructions per operation under QEMU -icount, lower is "
            "better, bars to scale within each operation. "
            + " ".join(f"{label}: x86_64 {x:,.0f}, AArch64 {a:,.0f}." for label, x, a in rows))
    s.text(24, 40, "Both architectures, in instructions", 20, weight=700)
    s.text(24, 64, "Retired instructions per operation under QEMU -icount; bars to scale "
           "within each operation", 14, fill=t["muted"])
    s.rect(24, 78, 14, 14, t["bar"], t["bar"], rx=3, sw=0)
    s.text(44, 90, "AArch64 (virt)", 14)
    s.rect(160, 78, 14, 14, t["other"], t["other"], rx=3, sw=0)
    s.text(180, 90, "x86_64 (microvm)", 14)
    y = top
    for label, x86, arm in rows:
        s.text(x0 - 12, y + 30, label, 15, weight=700, anchor="end")
        longest = max(x86, arm)
        for k, (v, c) in enumerate([(arm, t["bar"]), (x86, t["other"])]):
            by = y + 8 + k * 24
            w = max(3.0, v / longest * (x1 - x0))
            s.rect(x0, by, round(w, 1), 18, c, c, rx=3, sw=0)
            s.text(round(x0 + w + 8, 1), by + 14, f"{v:,.0f}", 14,
                   weight=700 if k == 0 else 400)
        y += gh
    s.text(24, h - 22, "Exact counts: the same on every run and host. GitHub's ARM64 runners "
           "have no KVM, so no AArch64 timings.", 13, fill=t["muted"])
    return s


# ---------------------------------------------------------------- highlights
# `laptop.memory_bytes` on Firecracker before snapshots referred to shared
# frames (PR #87, `add-shared-snapshots`): no longer measured, so quoted.
LAPTOP_FULL_COPY = 60_424_192


def rhizo_values(name):
    return [m["median"] for m in RESULTS["metrics"] if m["name"] == name and m["guest"] == "rhizo"]


def highlights(t):
    """The README's headline figures as cards, or None without them."""
    node_jit = median("workload.node_jit_cold_us", required=False)
    laptop = median("laptop.memory_bytes", required=False)
    loaded = median("snapshot.loaded_answer_us", vmm="qemu-microvm", required=False)
    cold_answer = median("snapshot.cold_answer_us", vmm="qemu-microvm", required=False)
    stall = median("cell.host_op_stall_us", required=False)
    stopped = median("cell.host_op_stall_stopped_us", required=False)
    wake = rhizo_values("density.wake_p99_us")
    ops = rhizo_values("density.ops_per_s")
    need = [NODE_WARM, NODE_COLD, node_jit, NP_WARM, LX_NP_WARM, laptop, loaded,
            cold_answer, stall, stopped, wake, ops]
    if not all(need):
        return None
    jit = round((1 - node_jit / NODE_COLD) * 100)
    cards = [
        ("Node.js, ready to answer", "forked from a warm template", us(NODE_WARM),
         [("warm fork", NODE_WARM, us(NODE_WARM), True),
          ("cold start", NODE_COLD, us(NODE_COLD), False)],
         f"with its JIT, cold is {jit}% faster"),
        ("numpy from PyPI wheels", "forked after import numpy", us(NP_WARM),
         [("Rhizo cell", NP_WARM, us(NP_WARM), True),
          ("Linux process", LX_NP_WARM, us(LX_NP_WARM), False)],
         f"cold: cell {us(NP_COLD)}, Linux {us(LX_NP_COLD)}"),
        ("50 agents on a laptop", "45 of them suspended", f"{laptop / 1e6:.1f} MB",
         [("shared snapshots", laptop, f"{laptop / 1e6:.1f} MB", True),
          ("full copies (before)", LAPTOP_FULL_COPY, f"{LAPTOP_FULL_COPY / 1e6:.0f} MB",
           False)],
         "snapshots point at shared pages"),
        ("A template saved to a file", "loaded into a fresh VM",
         f"{cold_answer / loaded:.1f}× sooner",
         [("loaded template", loaded, us(loaded), True),
          ("cold agent", cold_answer, us(cold_answer), False)],
         "to its first answer (QEMU)"),
        ("Host operation on one cell", "how long other cells stall", us(stall),
         [("now", stall, us(stall), True),
          ("every CPU stopped", stopped, us(stopped), False)],
         f"{stopped / stall:.0f}× shorter"),
        ("10,000 busy cells, 1 vCPU", "99th percentile wait to wake",
         f"{min(wake) / 1000:.1f}–{max(wake) / 1000:.1f} ms", None,
         f"{min(ops) / 1000:.0f}k–{max(ops) / 1000:.0f}k operations/s"),
    ]
    cw, ch, gap, top = 240, 196, 16, 84
    h = top + 2 * ch + gap + 44
    s = Svg(t, h, "The numbers that matter",
            "Six headline figures from one benchmark run. " + " ".join(
                f"{a}, {b}: {v}" + (" (" + ", ".join(f"{n} {lab}" for n, _, lab, _ in bars)
                                    + ")" if bars else "") + f"; {foot}."
                for a, b, v, bars, foot in cards))
    s.text(24, 40, "The numbers that matter", 20, weight=700)
    s.text(24, 64, "Medians from one CI run with KVM (Firecracker unless noted); "
           "bars to scale within each card", 14, fill=t["muted"])
    for i, (a, b, v, bars, foot) in enumerate(cards):
        x = 24 + (i % 3) * (cw + gap)
        y = top + (i // 3) * (ch + gap)
        s.rect(x, y, cw, ch, t["box"], t["boxline"], rx=14)
        s.text(x + 16, y + 26, a, 14.5, weight=700)
        s.text(x + 16, y + 45, b, 12.5, fill=t["muted"])
        s.text(x + 16, y + 84, v, 28, weight=700, fill=t["accent"])
        if bars:
            longest = max(val for _, val, _, _ in bars)
            for j, (name, val, lab, rh) in enumerate(bars):
                by = y + 112 + j * 32
                s.text(x + 16, by, name, 12.5, fill=t["text"] if rh else t["muted"])
                s.text(x + cw - 16, by, lab, 12.5, anchor="end", weight=700 if rh else 400,
                       fill=t["text"] if rh else t["muted"])
                c = t["bar"] if rh else t["other"]
                s.rect(x + 16, by + 6, round(max(3.0, val / longest * (cw - 32)), 1), 10,
                       c, c, rx=3, sw=0)
        else:
            s.text(x + 16, y + 116, "range over four VMMs,", 12.5, fill=t["muted"])
            s.text(x + 16, y + 136, "the slowest: Firecracker", 12.5, fill=t["muted"])
        s.text(x + 16, y + ch - 16, foot, 12.5, weight=700, fill=t["accent"])
    s.text(24, h - 18, "Shared runners are noisy: figures move between runs. The tables "
           "hold every value.", 12.5, fill=t["muted"])
    return s


# ---------------------------------------------------------------- roadmap
BOOT_MS = median("boot.time_to_ready_ms")
AGENT_FORK = median("workload.agent_fork_us", required=False)
DENSITY = median("density.cells", required=False)


def has(name, vmm="firecracker"):
    return median(name, vmm=vmm, required=False) is not None


# Stages a newcomer can follow. Each lists the internal milestones it groups
# (not drawn): a stage is done when every one of them has its metric (or
# document) in the results; otherwise it shows its planned state. Metric names
# of milestones still in progress are the ones their changes add.
STAGES = [
    ("The foundations", "Boots in milliseconds on Arm and x86, tested on every change",
     [True], "next", (f"{BOOT_MS:.0f} ms", "to boot (Firecracker)")),
    ("The cell", "Runs ordinary Linux programs, each sealed off from the rest",
     [True], "next", (f"{IDLE_KB:.0f} KB", "per idle cell")),
    ("Native operations", "Pause, resume or copy a running agent in microseconds",
     [True], "next", (us(FORK), "to fork a 50 MiB agent")),
    ("Real programs", "Threads, networking, Python and every processor core",
     [THREADS, LOOPBACK, CPY_WARM, SCALING], "next",
     (us(CPY_WARM), "a Python request, warm") if CPY_WARM else None),
    ("Proven with numbers", "Compared fairly with Linux, VMs and sandboxes; reviewed",
     [GV_RUN, CALL_AT == "same VM", SECURITY], "next",
     (times(RUN, LINUX_SPAWN).replace("≈ ", "") if LINUX_SPAWN else "",
      "to start a program")),
    ("A real agent", "Calls a model, explores branches, pauses between turns",
     [has("net.tcp_rtt_us"), has("cell.tcp_resume_us"), has("workload.agent_turn_us"),
      has("workload.agent_fork_us")], "now",
     (us(AGENT_FORK), "to fork a live agent three ways") if AGENT_FORK else None),
    ("Scale and compatibility", "Thousands of busy agents and everyday software, safely shared",
     [has("cell.shootdown_us"), has("density.wake_p99_us"),
      has("workload.dynamic_run_us"), has("workload.node_warm_us"),
      has("workload.numpy_warm_us"), has("cell.syscall_kpti_ns")], "now",
     (f"{DENSITY:,.0f}", "busy cells in one VM") if DENSITY else None),
    ("Families and modules", "Agents in walled families; abilities added as signed modules",
     [False], "next", None),
    ("The product", "Download a ready agent in two clicks, bring your own model",
     [False], "vision", None),
]


def roadmap(t):
    rows = [(name, line, "done" if all(parts) else planned, fig)
            for name, line, parts, planned, fig in STAGES]
    ndone = sum(1 for r in rows if r[2] == "done")
    rh, top = 62, 100
    h = top + rh * len(rows) + 16
    s = Svg(t, h, "Where Rhizo stands",
            f"{ndone} of {len(rows)} stages done. " + " ".join(
                f"{name} ({ {'done': 'done', 'now': 'in progress', 'next': 'next', 'vision': 'vision'}[k]}): "
                f"{line.lower()}" + (f", {f[0]} {f[1]}" if f and k == "done" else "") + "."
                for name, line, k, f in rows))
    s.text(24, 40, f"Where Rhizo stands: {ndone} of {len(rows)} stages done", 20, weight=700)
    for lx, kind, lab in [(24, "done", "Done"), (110, "now", "In progress"),
                          (236, "next", "Designed"), (350, "vision", "Vision")]:
        dot(s, t, lx + 9, 70, kind)
        s.text(lx + 24, 75, lab, 14, fill=t["muted"])
    rail = 52
    s.line(rail, top + rh / 2, rail, top + rh * (len(rows) - 0.5), color=t["boxline"],
           arrow=False, sw=2)
    y = top
    for name, line, kind, fig in rows:
        cy = y + rh / 2
        dot(s, t, rail, cy, kind)
        s.text(80, cy - 4, name, 16, weight=700,
               fill=t["warn"] if kind == "now" else
               t["muted"] if kind in ("next", "vision") else t["text"])
        s.text(80, cy + 17, line, 14, fill=t["muted"])
        if fig and kind == "done" and fig[0]:
            s.text(776, cy - 4, fig[0], 16, weight=700, fill=t["accent"], anchor="end")
            s.text(776, cy + 17, fig[1], 13, fill=t["muted"], anchor="end")
        elif kind == "now":
            s.text(776, cy - 4, "in progress", 14, weight=700, fill=t["warn"], anchor="end")
        elif kind == "vision":
            s.text(776, cy - 4, "vision", 14, weight=700, fill=t["muted"], anchor="end")
        elif kind == "next":
            s.text(776, cy - 4, "designed", 14, weight=700, fill=t["warn"], anchor="end")
            s.text(776, cy + 17, "docs/cell-hierarchy.md", 13, fill=t["muted"], anchor="end")
        y += rh
    return s


def dot(s, t, x, y, kind):
    if kind == "done":
        s.add(f'<circle cx="{x}" cy="{y}" r="9" fill="{t["done"]}"/>')
        s.add(f'<path d="M{x-4.5} {y}l3 3l6-6" fill="none" stroke="{t["donetext"]}" '
              'stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>')
    elif kind == "now":
        s.add(f'<circle cx="{x}" cy="{y}" r="9" fill="{t["warnfill"]}" '
              f'stroke="{t["warn"]}" stroke-width="2"/>')
        s.add(f'<path d="M{x} {y-7}A7 7 0 0 1 {x} {y+7}z" fill="{t["warn"]}"/>')
    elif kind == "vision":
        s.add(f'<circle cx="{x}" cy="{y}" r="8" fill="{t["bg"]}" '
              f'stroke="{t["pending"]}" stroke-width="1.5" stroke-dasharray="1.5 2.5"/>')
        s.add(f'<circle cx="{x}" cy="{y}" r="2.5" fill="{t["pending"]}"/>')
    else:
        s.add(f'<circle cx="{x}" cy="{y}" r="8" fill="{t["bg"]}" '
              f'stroke="{t["pending"]}" stroke-width="2" stroke-dasharray="3 2.5"/>')


for name, fn in [("highlights", highlights), ("measured", chart), ("scorecard", scorecard),
                 ("roadmap", roadmap), ("arches", arches)]:
    both(name, fn)
print("ok")
