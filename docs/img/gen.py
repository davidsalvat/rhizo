#!/usr/bin/env python3
"""Generate the README graphics (light and dark SVGs) in docs/img/ from one
benchmark run, so the charts agree with the README's generated tables:

    python3 docs/img/gen.py bench-results.json

`bench-results.json` is what `cargo xtask bench --output` writes (CI
publishes it as the `bench-results-N` annotations); regenerate the tables
from the same file with `cargo xtask bench --from bench-results.json
--readme`. Figures use the Firecracker column; ranges span every VMM.
"""
import json
import math
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent
RESULTS = json.load(open(sys.argv[1]))


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
    """'≈ N× faster', N to two significant figures."""
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
SECURITY = (OUT.parent / "security.md").exists()
PY_COLD = median("workload.python_cold_us")
PY_WARM = median("workload.python_warm_us")
IDLE_KB = median("cell.idle_bytes", vmm=None) / 1000
FONT = "Inter, -apple-system, Segoe UI, Helvetica, Arial, sans-serif"
W = 800

THEMES = {
    "light": dict(
        bg="#ffffff", border="#d0d7de", text="#0e2a20", muted="#4b5c55",
        box="#f3f5f4", boxline="#c3ccc8",
        cell="#eafaf2", cellline="#1f9d6b", accent="#146c4a",
        bar="#1f9d6b", other="#8c959f", warn="#875a00", warnfill="#fff4d6",
        done="#16784f", donetext="#ffffff", arrow="#57706a", pending="#8c959f",
    ),
    "dark": dict(
        bg="#0d1117", border="#30363d", text="#e6edf3", muted="#9ba7b3",
        box="#161b22", boxline="#3d444d",
        cell="#0f2a1f", cellline="#63d4a0", accent="#63d4a0",
        bar="#63d4a0", other="#768390", warn="#e3b341", warnfill="#2e2513",
        done="#63d4a0", donetext="#0e2a20", arrow="#8b98a5", pending="#768390",
    ),
}


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


class Svg:
    def __init__(self, t, h, title, desc):
        self.t, self.h, self.parts = t, h, []
        self.head = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {h}" '
            f'width="{W}" height="{h}" role="img" font-family="{FONT}">\n'
            f"<title>{esc(title)}</title>\n<desc>{esc(desc)}</desc>\n"
            '<defs><marker id="a" viewBox="0 0 10 10" refX="9" refY="5" '
            'markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
            f'<path d="M0 0L10 5L0 10z" fill="{t["arrow"]}"/></marker></defs>\n'
            f'<rect class="bg" x="0.5" y="0.5" width="{W-1}" height="{h-1}" rx="12" '
            f'fill="{t["bg"]}" stroke="{t["border"]}"/>\n'
        )

    def add(self, s):
        self.parts.append(s)

    def rect(self, x, y, w, h, fill, stroke, rx=8, dash=False, sw=1.5):
        d = ' stroke-dasharray="5 4"' if dash else ""
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" '
                 f'fill="{fill}" stroke="{stroke}" stroke-width="{sw}"{d}/>')

    def text(self, x, y, s, size=16, fill=None, weight=400, anchor="start"):
        fill = fill or self.t["text"]
        a = f' text-anchor="{anchor}"' if anchor != "start" else ""
        w = f' font-weight="{weight}"' if weight != 400 else ""
        self.add(f'<text x="{x}" y="{y}" font-size="{size}" fill="{fill}"{w}{a}>{esc(s)}</text>')

    def line(self, x1, y1, x2, y2, color=None, arrow=True, sw=1.8, dash=False):
        color = color or self.t["arrow"]
        m = ' marker-end="url(#a)"' if arrow else ""
        d = ' stroke-dasharray="4 4"' if dash else ""
        self.add(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{color}" '
                 f'stroke-width="{sw}"{m}{d}/>')

    def path(self, d, color=None, arrow=True, sw=1.8):
        color = color or self.t["arrow"]
        m = ' marker-end="url(#a)"' if arrow else ""
        self.add(f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{sw}"{m}/>')

    def save(self, name):
        (OUT / name).write_text(self.head + "\n".join(self.parts) + "\n</svg>\n")


# ---------------------------------------------------------------- stack
def stack(t):
    s = Svg(t, 470, "The stack today versus Rhizo",
            "Today an agent runs on orchestrator, host Linux, a VMM or userspace "
            "kernel and often a guest Linux. With Rhizo, cells run directly on "
            "a kernel whose operations are create, suspend, resume and fork.")
    lx, rx, cw = 32, 424, 344
    s.text(lx, 46, "Today", 20, weight=700)
    s.text(rx, 46, "With Rhizo", 20, fill=t["accent"], weight=700)
    s.line(400, 30, 400, 400, color=t["border"], arrow=False, sw=1)
    layers = ["Orchestrator", "Host Linux", "VMM or userspace kernel",
              "Guest Linux (often)", "Agent code"]
    y, bh, gap = 70, 48, 20
    for i, name in enumerate(layers):
        last = i == len(layers) - 1
        s.rect(lx, y, cw, bh, t["cell"] if last else t["box"],
               t["cellline"] if last else t["boxline"])
        s.text(lx + cw / 2, y + 30, name, 17, anchor="middle",
               weight=600 if last else 400)
        if not last:
            s.line(lx + cw / 2, y + bh, lx + cw / 2, y + bh + gap - 2)
        y += bh + gap
    # Rhizo side
    s.rect(rx, 70, cw, bh, t["box"], t["boxline"])
    s.text(rx + cw / 2, 100, "Orchestrator", 17, anchor="middle")
    s.line(rx + cw / 2, 118, rx + cw / 2, 136)
    s.rect(rx, 138, cw, 84, t["cell"], t["cellline"], sw=2)
    s.text(rx + cw / 2, 170, "Rhizo kernel", 19, weight=700, anchor="middle")
    s.text(rx + cw / 2, 198, "create · suspend · resume · fork", 15,
           fill=t["muted"], anchor="middle")
    cy = 262
    xs = [rx + 6, rx + 120, rx + 234]
    for x in xs:
        s.line(x + 52, 222, x + 52, cy - 2)
        s.rect(x, cy, 104, 76, t["cell"], t["cellline"], rx=38)
        s.text(x + 52, cy + 34, "cell", 16, weight=600, anchor="middle")
        s.text(x + 52, cy + 55, "agent code", 14, fill=t["muted"], anchor="middle")
    s.text(rx + cw / 2, 370, "thousands per node, most asleep", 14,
           fill=t["muted"], anchor="middle")
    s.text(lx, 432, "Snapshots, clones and isolation", 15, fill=t["muted"])
    s.text(lx, 454, "are added from the outside.", 15, fill=t["muted"])
    s.text(rx, 432, "Snapshot, resume and fork are", 15, fill=t["muted"])
    s.text(rx, 454, "native operations of the kernel.", 15, fill=t["muted"])
    return s


# ---------------------------------------------------------------- lifecycle
def lifecycle(t):
    s = Svg(t, 440, "What a cell can do",
            "A cell is created from a static executable. A running cell can be "
            "suspended into a snapshot and resumed, forked into copy-on-write "
            "children, or frozen as a template that new cells start from.")
    # executable
    s.rect(24, 168, 150, 64, t["box"], t["boxline"])
    s.text(99, 196, "Static Linux", 15, anchor="middle")
    s.text(99, 216, "executable", 15, anchor="middle")
    s.line(174, 200, 248, 200)
    s.text(211, 190, "create", 14, fill=t["muted"], anchor="middle", weight=600)
    # running cell
    s.rect(250, 160, 220, 80, t["cell"], t["cellline"], rx=24, sw=2)
    s.text(360, 196, "Running cell", 17, weight=700, anchor="middle")
    s.text(360, 218, "memory · registers · files", 14, fill=t["muted"], anchor="middle")
    # snapshot (top)
    s.rect(290, 24, 140, 56, t["box"], t["boxline"], dash=True)
    s.text(360, 57, "Snapshot", 16, weight=600, anchor="middle")
    s.line(330, 158, 330, 82)
    s.line(390, 82, 390, 158)
    s.text(320, 125, "suspend", 14, fill=t["muted"], anchor="end", weight=600)
    s.text(400, 116, "resume", 14, fill=t["muted"], weight=600)
    s.text(400, 136, us(RESUME), 14, fill=t["accent"], weight=700)
    # children (right)
    s.line(470, 200, 562, 200)
    s.text(516, 190, "fork(n)", 14, fill=t["muted"], anchor="middle", weight=600)
    s.text(516, 222, us(FORK), 14, fill=t["accent"], anchor="middle", weight=700)
    for i, (dx, dy) in enumerate([(0, 0), (12, 12), (24, 24)][::-1]):
        s.rect(564 + dx, 150 + dy, 170, 64, t["cell"], t["cellline"], rx=32)
    s.text(564 + 85, 150 + 30, "Children", 16, weight=600, anchor="middle")
    s.text(564 + 85, 150 + 50, "copy-on-write", 14, fill=t["muted"], anchor="middle")
    s.text(661, 266, "share pages until one writes", 14, fill=t["muted"],
           anchor="middle")
    # template (bottom)
    s.line(360, 242, 360, 320)
    s.text(370, 286, "freeze", 14, fill=t["muted"], weight=600)
    s.rect(290, 322, 140, 56, t["box"], t["boxline"], dash=True)
    s.text(360, 355, "Template", 16, weight=600, anchor="middle")
    s.line(430, 350, 562, 350)
    s.text(496, 340, "start", 14, fill=t["muted"], anchor="middle", weight=600)
    s.text(496, 372, us(TEMPLATE), 14, fill=t["accent"], anchor="middle", weight=700)
    s.rect(564, 318, 170, 64, t["cell"], t["cellline"], rx=32)
    s.text(649, 346, "New cells", 16, weight=600, anchor="middle")
    s.text(649, 366, "already warmed up", 14, fill=t["muted"], anchor="middle")
    s.text(24, 420, "Times: medians on a GitHub-hosted runner with KVM (see the chart below).",
           14, fill=t["muted"])
    return s


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
    groups = fair + whole
    x0, x1 = 200, 676
    lo = -1
    hi = math.ceil(math.log10(max(v for g in groups for _, v, _, _ in g[2])))

    def X(v):
        return x0 + (math.log10(v) - lo) / (hi - lo) * (x1 - x0)

    def gheight(g):
        return 30 + 32 * len(g[2]) + 18

    top = 104
    tier_h = 44
    bottom = top + sum(tier_h + sum(gheight(g) for g in gs) for _, gs in tiers)
    h = bottom + 108
    s = Svg(t, h, "Measured on the same machine",
            "Log-scale bar chart, lower is better. "
            + " ".join(f"{name}. " + " ".join(
                f"{g[0]}: " + ", ".join(f"{n} {lab}" for n, _, lab, _ in g[2]) + "."
                for g in gs) for name, gs in tiers))
    s.text(24, 40, "Measured on the same machine", 20, weight=700)
    s.text(24, 64, "Time per operation, log scale (each line is 10× the previous one); "
           "lower is better", 14, fill=t["muted"])
    s.rect(24, 78, 14, 14, t["bar"], t["bar"], rx=3, sw=0)
    s.text(44, 90, "Rhizo", 14)
    s.rect(110, 78, 14, 14, t["other"], t["other"], rx=3, sw=0)
    s.text(130, 90, "Existing mechanism, or a cold start", 14)
    names = ["0.1 µs", "1 µs", "10 µs", "100 µs", "1 ms", "10 ms", "100 ms", "1 s"]
    ticks = [(10.0 ** e, names[e + 1]) for e in range(lo, hi + 1)]
    s.line(x0, bottom, x1, bottom, color=t["boxline"], arrow=False, sw=1)
    for v, lab in ticks:
        s.line(X(v), bottom, X(v), bottom + 5, color=t["boxline"], arrow=False, sw=1)
        s.text(X(v), bottom + 20, lab, 14, fill=t["muted"], anchor="middle")
    y = top
    for tier, gs in tiers:
        s.line(24, y + 14, 776, y + 14, color=t["boxline"], arrow=False, sw=1)
        s.text(24, y + 34, tier, 15, fill=t["accent"], weight=700)
        y += tier_h
        for title, rt, bars in gs:
            s.text(24, y + 22, title, 15, weight=700)
            if rt:
                s.text(776, y + 22, rt, 15, fill=t["accent"], weight=700, anchor="end")
            by = y + 34
            for v, _ in ticks:
                s.line(X(v), by, X(v), by + 32 * len(bars), color=t["border"], arrow=False,
                       sw=1)
            for name, v, lab, rh in bars:
                c = t["bar"] if rh else t["other"]
                s.text(x0 - 10, by + 17, name, 14, fill=t["text"] if rh else t["muted"],
                       anchor="end")
                s.rect(x0, by + 3, round(X(v) - x0, 1), 20, c, c, rx=3, sw=0)
                s.add(f'<text x="{X(v) + 8}" y="{by + 18}" font-size="14" fill="{t['text']}" '
                      f'font-weight="{700 if rh else 400}" stroke="{t['bg']}" '
                      f'stroke-width="5" stroke-linejoin="round" paint-order="stroke">'
                      f'{lab}</text>')
                by += 32
            y += gheight((title, rt, bars))
    cy = bottom + 50
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
            s.rect(x + 18, 180, 74, 28, t["done"], t["done"], rx=14, sw=0)
            s.text(x + 55, 199, "✓ Met", 14, fill=t["donetext"], weight=700,
                   anchor="middle")
        else:
            s.rect(x + 18, 180, 112, 28, t["warnfill"], t["warn"], rx=14, sw=1.5)
            s.text(x + 74, 199, "In progress", 14, fill=t["warn"], weight=700,
                   anchor="middle")
        x += cw + gap
    return s


# ---------------------------------------------------------------- roadmap
def roadmap(t):
    done = [
        ("H0", "Boot and test harness", "QEMU, serial console, in-kernel tests"),
        ("H1", "Memory, paging and heap", "higher half, W^X, CPU exceptions"),
        ("H1.5", "Fast boot and benchmarks", "Firecracker, Cloud Hypervisor, CI gates"),
        ("H1.6", "AArch64 as primary", "x86_64 kept as the second architecture"),
        ("H2", "First cell", "user mode, ELF loader, Linux system calls"),
        ("H3", "Scheduler and isolation", "address-space tags (ASIDs, PCIDs)"),
        ("H4", "Suspend and resume", "lazy restore, pages back on first touch"),
        ("H5", "Fork and templates", "copy-on-write down to the page tables"),
        ("H6", "Real glibc programs", "differential tests against Linux"),
        ("H7", "Comparisons", "against Firecracker and Linux, same machine"),
        ("H8", "Files per cell", "carried by snapshots, forks and templates"),
        ("H9", "Memory accounting, density", f"1,000 idle cells, {IDLE_KB:.0f} KB each"),
        ("H10", "A real interpreter", "MicroPython as a cell, warm templates"),
    ] + ([("H11", "gVisor comparison", "userspace-kernel sandboxes, same machine")]
         if GV_RUN else []) + \
        ([("H12", "Fair comparisons", "Linux processes in the same VM, CRIU")]
         if CALL_AT == "same VM" else []) + \
        ([("H13", "Adversarial security review", "12 findings fixed, docs/security.md")]
         if SECURITY else []) + \
        ([("H14", "Threads", f"pthreads, futexes; a thread in {us(THREADS)}")]
         if THREADS else [])
    now = []
    nxt = [("next", name, "") for name, done_ in [
        ("gVisor comparison", GV_RUN), ("Fair comparisons", CALL_AT == "same VM"),
        ("Adversarial security review", SECURITY), ("Threads", THREADS),
        ("Sockets and network", None), ("CPython in a cell", None), ("Multi-core", None),
        ("Dedicated benchmark machine", None)] if not done_]
    rows = [(r, "done") for r in done] + [(r, "now") for r in now] + \
        [(r, "next") for r in nxt]
    rh, top = 34, 92
    h = top + rh * len(rows) + 20
    s = Svg(t, h, "Roadmap status",
            f"{len(done)} milestones done, from the boot and test harness to "
            f"{done[-1][1].lower()}. Next: "
            + ", ".join(n for _, n, _ in nxt).lower() + ".")
    s.text(24, 40, f"Proof of concept: {len(done)} milestones done", 20, weight=700)
    # legend
    for lx, kind, lab in [(24, "done", "Done"), (110, "next", "Next")]:
        dot(s, t, lx + 9, 66, kind)
        s.text(lx + 24, 71, lab, 14, fill=t["muted"])
    rail = 52
    s.line(rail, top + rh / 2, rail, top + rh * (len(rows) - 0.5), color=t["border"],
           arrow=False, sw=2)
    y = top
    for (code, name, note), kind in rows:
        cy = y + rh / 2
        dot(s, t, rail, cy, kind)
        if code != "next":
            s.text(76, cy + 5.5, code, 15, weight=700,
                   fill=t["warn"] if kind == "now" else t["accent"])
        s.text(76 if code == "next" else 128, cy + 5.5, name, 15,
               weight=700 if kind == "now" else 400,
               fill=t["muted"] if kind == "next" else t["text"])
        if note:
            s.text(416, cy + 5.5, note, 14, fill=t["muted"])
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
    else:
        s.add(f'<circle cx="{x}" cy="{y}" r="8" fill="{t["bg"]}" '
              f'stroke="{t["pending"]}" stroke-width="2" stroke-dasharray="3 2.5"/>')


for name, fn in [("stack", stack), ("lifecycle", lifecycle), ("measured", chart),
                 ("scorecard", scorecard), ("roadmap", roadmap)]:
    for theme, t in THEMES.items():
        fn(t).save(f"{name}{'-dark' if theme == 'dark' else ''}.svg")
print("ok")
