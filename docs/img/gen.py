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
    ] + ([
        ("CPython agent request, cold start", ratio(CPY_COLD, LX_CPY_COLD),
         [("Rhizo cell", CPY_COLD, us(CPY_COLD), True)]
         + linux("process", LX_CPY_COLD, "same VM")),
        ("CPython agent request, from a warm template", ratio(CPY_WARM, LX_CPY_WARM),
         [("Rhizo cell", CPY_WARM, us(CPY_WARM), True)]
         + linux("fork of warm", LX_CPY_WARM, "same VM")),
    ] if CPY_COLD and CPY_WARM else []) + [
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
BOOT_MS = median("boot.time_to_ready_ms")


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
     [has("net.tcp_rtt_us"), has("net.tcp_rtt_us", "cloud-hypervisor"),
      has("cell.tcp_resume_us"), has("workload.agent_turn_us")], "now", None),
    ("Scale and compatibility", "Thousands of busy agents and everyday software, safely shared",
     [has("cell.shootdown_us"), has("density.wake_p99_us"),
      has("workload.dynamic_run_us"), has("workload.node_warm_us")], "next", None),
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
    s.text(24, 40, f"Proof of concept: {ndone} of {len(rows)} stages done", 20, weight=700)
    for lx, kind, lab in [(24, "done", "Done"), (110, "now", "In progress"),
                          (236, "next", "Next"), (318, "vision", "Vision")]:
        dot(s, t, lx + 9, 70, kind)
        s.text(lx + 24, 75, lab, 14, fill=t["muted"])
    rail = 52
    s.line(rail, top + rh / 2, rail, top + rh * (len(rows) - 0.5), color=t["border"],
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
        y += rh
    return s


# ---------------------------------------------------------------- vision
def tag(s, t, x, y, planned, word="measured"):
    """A small 'planned' (or `word`) label ending at x."""
    s.text(x, y, "planned" if planned else word, 12, weight=700,
           fill=t["muted"] if planned else t["accent"], anchor="end")


def layers(t):
    s = Svg(t, 488, "Simple on top, deep below",
            "Layers of Rhizo. On top, what the user sees: a two-click app and a "
            "template catalog (both planned) and their agents. A line separates "
            "it from what the user does not see: agent cells that are created, "
            f"suspended, resumed and forked in microseconds (fork {us(FORK)}, "
            f"resume {us(RESUME)}, measured), and below them the Rhizo kernel, "
            "which rebuilds the foundations: memory, scheduler, isolation, "
            "network and snapshots.")
    s.text(24, 40, "Simple on top, deep below", 20, weight=700)
    s.text(24, 64, "The user sees two clicks; the kernel does the hard work.", 14,
           fill=t["muted"])
    boxes = [("Two-click app", "pick, open, done", True),
             ("Template catalog", "ready environments", True),
             ("Your agents", "Python, tools, files", False)]
    for i, (name, sub, planned) in enumerate(boxes):
        x = 24 + i * 256
        s.rect(x, 92, 240, 72, t["box"], t["boxline"], dash=planned)
        s.text(x + 16, 122, name, 16, weight=700)
        s.text(x + 16, 146, sub, 14, fill=t["muted"])
        tag(s, t, x + 228, 122, planned, "built")
    s.line(24, 192, 776, 192, color=t["boxline"], arrow=False, sw=1.5, dash=True)
    s.text(24, 184, "what you see", 13, fill=t["muted"], weight=600)
    s.text(24, 210, "what you don't", 13, fill=t["muted"], weight=600)
    s.rect(24, 224, 752, 84, t["cell"], t["cellline"], rx=24, sw=2)
    s.text(48, 258, "Agent cells", 18, weight=700)
    s.text(48, 284, "create · suspend · resume · fork", 14, fill=t["muted"])
    s.text(752, 258, f"fork {us(FORK)} · resume {us(RESUME)}", 16, weight=700,
           fill=t["accent"], anchor="end")
    tag(s, t, 752, 284, False)
    s.line(400, 310, 400, 332)
    s.rect(24, 334, 752, 116, t["box"], t["boxline"])
    s.text(48, 364, "Rhizo kernel: the foundations, rebuilt for agents", 17, weight=700)
    parts = ["Memory", "Scheduler", "Isolation", "Network", "Snapshots"]
    bw = (720 - 4 * 12) / 5
    for i, name in enumerate(parts):
        x = 40 + i * (bw + 12)
        s.rect(round(x, 1), 380, round(bw, 1), 52, t["bg"], t["boxline"], rx=6)
        s.text(round(x + bw / 2, 1), 412, name, 15, anchor="middle")
    s.text(24, 476, "Solid: built and measured. Dashed: planned.", 13, fill=t["muted"])
    return s


def clicks(t):
    steps = [
        ("Pick a template", "a ready Python, data or Node agent", None, None),
        ("Download it", "one compressed snapshot", None, None),
        ("Open an agent", "the snapshot resumes", us(RESUME), "resume from memory"),
        ("Open ten more", "ten forks, no extra memory until they write",
         us(FORK), "per fork"),
        ("Suspend, come back later", "the agent waits as a snapshot",
         f"{IDLE_KB:.0f} KB", "per idle agent"),
    ]
    rh, top = 76, 92
    h = top + rh * len(steps) + 40
    s = Svg(t, h, "Two clicks",
            "The user's path: " + " ".join(
                f"{i + 1}. {n}: {d}" + (f" ({v} {c}, measured)." if v else " (planned).")
                for i, (n, d, v, c) in enumerate(steps)))
    s.text(24, 40, "Two clicks", 20, weight=700)
    s.text(24, 64, "What opening agents looks like for the user.", 14, fill=t["muted"])
    s.line(52, top + 32, 52, top + rh * (len(steps) - 1) + 32, color=t["border"],
           arrow=False, sw=2)
    for i, (name, desc, value, cap) in enumerate(steps):
        y = top + i * rh
        s.rect(88, y, 688, 64, t["box"] if value is None else t["cell"],
               t["boxline"] if value is None else t["cellline"], dash=value is None)
        s.add(f'<circle cx="52" cy="{y + 32}" r="15" fill="{t["bg"]}" '
              f'stroke="{t["cellline"] if value else t["pending"]}" stroke-width="2"/>')
        s.text(52, y + 37, str(i + 1), 14, weight=700, anchor="middle")
        s.text(108, y + 28, name, 16, weight=700)
        s.text(108, y + 50, desc, 14, fill=t["muted"])
        if value:
            s.text(756, y + 28, value, 16, weight=700, fill=t["accent"], anchor="end")
            s.text(756, y + 50, cap, 13, fill=t["muted"], anchor="end")
        else:
            tag(s, t, 756, y + 28, True)
    s.text(24, h - 16, "Solid: built and measured. Dashed: planned.", 13, fill=t["muted"])
    return s


def model(t):
    s = Svg(t, 470, "The model beside Rhizo",
            "Local architecture. On the user's computer, any model runtime "
            "(Ollama, llama.cpp, vLLM) runs on the host GPU or NPU with one shared "
            "copy of the weights. Next to it, the Rhizo VM runs agent cells, "
            "including a fork, and each agent keeps its own state in its cell. "
            "Cells talk to the model over a local socket, and the Rhizo app "
            "coordinates both. The app and the socket are planned.")
    s.text(24, 40, "The model beside Rhizo", 20, weight=700)
    s.text(24, 64, "Any model, any runtime, full GPU speed; agents in cells.", 14,
           fill=t["muted"])
    s.rect(24, 80, 752, 360, t["bg"], t["border"], rx=12)
    s.text(40, 104, "Your computer (Linux, macOS or Windows)", 14, fill=t["muted"],
           weight=600)
    s.rect(40, 118, 720, 50, t["box"], t["boxline"], dash=True)
    s.text(60, 149, "Rhizo app: starts the VM, picks the model, manages agents", 15,
           weight=600)
    tag(s, t, 744, 149, True)
    s.line(160, 168, 160, 196)
    s.line(580, 168, 580, 196)
    # model side
    s.rect(40, 198, 240, 88, t["box"], t["boxline"])
    s.text(56, 226, "Model runtime", 16, weight=700)
    s.text(56, 248, "Ollama, llama.cpp, vLLM…", 14, fill=t["muted"])
    s.text(56, 270, "weights: one shared copy", 14, fill=t["muted"])
    s.line(160, 286, 160, 316, arrow=False)
    s.rect(40, 318, 240, 56, t["box"], t["boxline"])
    s.text(160, 351, "GPU / NPU", 16, weight=700, anchor="middle")
    # socket
    s.line(290, 242, 398, 242, dash=True)
    s.line(398, 262, 290, 262, dash=True)
    s.text(344, 230, "local socket", 13, fill=t["muted"], anchor="middle", weight=600)
    s.text(344, 284, "planned", 12, fill=t["muted"], anchor="middle", weight=700)
    # Rhizo VM
    s.rect(400, 198, 360, 226, t["cell"], t["cellline"], rx=16, sw=2)
    s.text(416, 224, "Rhizo VM", 16, weight=700)
    s.text(416, 246, "on KVM today; macOS and Windows planned", 13, fill=t["muted"])
    for i, (name, sub) in enumerate([("agent A", "cell"), ("agent B", "cell"),
                                     ("agent B′", "fork of B")]):
        x = 416 + i * 112
        s.rect(x, 262, 104, 64, t["bg"], t["cellline"], rx=28)
        s.text(x + 52, 290, name, 15, weight=700, anchor="middle")
        s.text(x + 52, 310, sub, 13, fill=t["muted"], anchor="middle")
    s.text(416, 356, "Each agent's state (program, history,", 13, fill=t["muted"])
    s.text(416, 376, "files) lives in its cell: that is what", 13, fill=t["muted"])
    s.text(416, 396, "Rhizo suspends, resumes and forks.", 13, fill=t["muted"])
    s.text(24, 460, "Solid: built today. Dashed: planned.", 13, fill=t["muted"])
    return s


def markets(t):
    s = Svg(t, 340, "Two markets, one engine",
            "Servers (B2B): density, cost per agent, an API. Local AI (B2C): an "
            "app, downloadable templates, your own model. Both products are "
            "planned; they run on the same engine, the Rhizo kernel and its "
            "agent cells, which is built today.")
    s.text(24, 40, "Two markets, one engine", 20, weight=700)
    for x, title, lines in [
            (24, "Servers (B2B)", ["many agents per machine", "low cost per agent",
                                   "an API to create, fork, resume"]),
            (412, "Your computer (B2C)", ["a two-click app", "downloadable templates",
                                          "your own local model"])]:
        s.rect(x, 64, 364, 140, t["box"], t["boxline"], dash=True)
        s.text(x + 20, 94, title, 17, weight=700)
        tag(s, t, x + 348, 94, True)
        for i, ln in enumerate(lines):
            s.text(x + 20, 124 + i * 24, "· " + ln, 14, fill=t["muted"])
        s.line(x + 182, 204, x + 182, 236)
    s.rect(24, 238, 752, 68, t["cell"], t["cellline"], rx=20, sw=2)
    s.text(400, 268, "One engine: the Rhizo kernel and its agent cells", 17, weight=700,
           anchor="middle")
    s.text(400, 291, "density, security and compatibility serve both", 14,
           fill=t["muted"], anchor="middle")
    s.text(24, 322, "Solid: built today. Dashed: planned.", 13, fill=t["muted"])
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


for name, fn in [("stack", stack), ("lifecycle", lifecycle), ("measured", chart),
                 ("scorecard", scorecard), ("roadmap", roadmap), ("layers", layers),
                 ("clicks", clicks), ("model", model), ("markets", markets)]:
    for theme, t in THEMES.items():
        fn(t).save(f"{name}{'-dark' if theme == 'dark' else ''}.svg")
print("ok")
