#!/usr/bin/env python3
"""Draw the README's hand-designed diagrams (light and dark SVGs) in
docs/img/. They hold no measurements, so they need no benchmark run:

    python3 docs/img/diagrams.py

The charts drawn from a benchmark run are `gen.py`'s. Both share the style
of `svg.py`.
"""
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the images
from svg import Svg, both  # noqa: E402


# ---------------------------------------------------------------- icons
# Each icon is drawn on a 24 x 24 grid, centred on (x, y), `size` wide.
def icon(s, name, x, y, size=24, color=None, sw=2, fill=None):
    t = s.t
    c = color or t["text"]
    k = size / 24
    f = fill or "none"
    body = {
        "bolt": f'<path d="M13 2L4 14h7l-1 8 9-12h-7z" fill="{c}" stroke="none"/>',
        "pause": f'<rect x="6" y="5" width="4" height="14" rx="1" fill="{c}"/>'
                 f'<rect x="14" y="5" width="4" height="14" rx="1" fill="{c}"/>',
        "play": f'<path d="M7 4l13 8-13 8z" fill="{c}" stroke="none"/>',
        "resume": f'<path d="M20 12a8 8 0 1 1-2.3-5.7"/><path d="M20 3v5h-5"/>'
                  f'<path d="M10 9l5 3-5 3z" fill="{c}" stroke="none"/>',
        "fork": '<circle cx="6" cy="5" r="2.5"/><circle cx="6" cy="19" r="2.5"/>'
                '<circle cx="18" cy="5" r="2.5"/><path d="M6 7.5v9"/>'
                '<path d="M18 7.5c0 5-12 4-12 9"/>',
        "shield": f'<path d="M12 2l8 3v6c0 5-3.4 9-8 11-4.6-2-8-6-8-11V5z" fill="{f}"/>'
                  '<path d="M8.5 12l2.5 2.5 4.5-5"/>',
        "lock": '<rect x="5" y="11" width="14" height="10" rx="2"/>'
                '<path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
        "download": '<path d="M12 3v12"/><path d="M7 10l5 5 5-5"/><path d="M4 20h16"/>',
        "chip": '<rect x="6" y="6" width="12" height="12" rx="2"/>'
                '<rect x="9.5" y="9.5" width="5" height="5" rx="1"/>'
                '<path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>',
        "star": f'<path d="M12 2.5l2.9 6 6.6.8-4.9 4.5 1.3 6.5L12 17l-5.9 3.3 1.3-6.5-4.9-4.5 '
                f'6.6-.8z" fill="{c}" stroke="none"/>',
        "save": '<path d="M5 3h11l3 3v15H5z"/><path d="M8 3v5h7V3"/>'
                '<rect x="8" y="13" width="8" height="6" rx="1"/>',
        "file": '<path d="M6 2h8l4 4v16H6z"/><path d="M14 2v4h4"/>',
        "snow": '<path d="M12 2v20M3.3 7l17.4 10M3.3 17L20.7 7"/>'
                '<path d="M9.5 3.5L12 6l2.5-2.5M9.5 20.5L12 18l2.5 2.5"/>',
        "spark": f'<path d="M12 2l2 7 7 2-7 2-2 7-2-7-7-2 7-2z" fill="{c}" stroke="none"/>',
        "robot": f'<rect x="4" y="7" width="16" height="13" rx="4" fill="{f}"/>'
                 '<path d="M12 7V3.5"/><circle cx="12" cy="3" r="1.3"/>'
                 f'<circle cx="9" cy="13" r="1.6" fill="{c}" stroke="none"/>'
                 f'<circle cx="15" cy="13" r="1.6" fill="{c}" stroke="none"/>'
                 '<path d="M9.5 17h5"/>',
        "laptop": '<rect x="4" y="4" width="16" height="11" rx="1.5"/><path d="M2 19h20l-2-4H4z"/>',
        "server": '<rect x="3" y="3" width="18" height="7" rx="1.5"/>'
                  '<rect x="3" y="14" width="18" height="7" rx="1.5"/>'
                  f'<circle cx="7" cy="6.5" r="1" fill="{c}"/><circle cx="7" cy="17.5" r="1" fill="{c}"/>',
        "flask": '<path d="M9 2h6M10 2v6L4.5 19a2 2 0 0 0 1.8 3h11.4a2 2 0 0 0 1.8-3L14 8V2"/>'
                 '<path d="M7 15h10"/>',
        "home": '<path d="M3 11l9-8 9 8"/><path d="M5 9.5V21h14V9.5"/><path d="M10 21v-6h4v6"/>',
        "globe": '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/>'
                 '<path d="M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18"/>',
        "cross": '<path d="M6 6l12 12M18 6L6 18"/>',
        "check": '<path d="M4 12.5l5 5L20 6.5"/>',
        "key": '<circle cx="8" cy="15" r="4.5"/><path d="M11.5 12L20 3.5M16 7.5l3 3M18.5 5l2 2"/>',
        "cube": '<path d="M12 2l9 5v10l-9 5-9-5V7z"/><path d="M3 7l9 5 9-5M12 12v10"/>',
        "replay": '<path d="M4 12a8 8 0 1 0 2.3-5.7"/><path d="M4 3v5h5"/>',
        "flag": '<path d="M5 22V3"/><path d="M5 4h12l-2.5 4L17 12H5"/>',
        "cpu": '<rect x="5" y="5" width="14" height="14" rx="2"/>'
               '<path d="M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3"/>',
        "eye": '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>',
        "user": '<circle cx="12" cy="7" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/>',
    }[name]
    s.add(f'<g transform="translate({x - size / 2:g} {y - size / 2:g}) scale({k:g})" '
          f'fill="none" stroke="{c}" stroke-width="{sw / k * k:g}" stroke-linecap="round" '
          f'stroke-linejoin="round">{body}</g>')


def badge(s, x, y, r, name, fill, color):
    s.circle(x, y, r, fill)
    icon(s, name, x, y, r * 1.15, color)


def pod(s, x, y, w, h, title, sub=None, dim=False, gold=False, size=15, rx=None):
    """A cell: a rounded green box with a title and an optional second line."""
    t = s.t
    stroke = t["gold"] if gold else (t["pending"] if dim else t["cellline"])
    s.rect(x, y, w, h, t["box"] if dim else t["cell"], stroke,
           rx=rx if rx is not None else min(h / 2, 22), sw=2.5 if gold else 2)
    cy = y + h / 2 + (-3 if sub else 5)
    s.text(x + w / 2, cy, title, size, weight=700, anchor="middle",
           fill=t["muted"] if dim else t["text"])
    if sub:
        s.text(x + w / 2, cy + 18, sub, 12.5, anchor="middle", fill=t["muted"])


def tagline(s, x, y, txt, kind="designed"):
    """A small rounded tag ending at x: 'designed, not built yet' and the like."""
    t = s.t
    w = 7.4 * len(txt) + 28
    fill, col = (t["warnfill"], t["warn"]) if kind == "designed" else (t["cell"], t["accent"])
    s.rect(x - w, y, w, 42, fill, col, rx=21, sw=1.2)
    s.text(x - w / 2, y + 26, txt, 12.5, weight=700, fill=col, anchor="middle")


def heading(s, title, sub=None):
    s.text(24, 40, title, 20, weight=700)
    if sub:
        s.text(24, 64, sub, 14, fill=s.t["muted"])


# ---------------------------------------------------------------- hero
def hero(t):
    s = Svg(t, 450, "Rhizo: a home for AI agents",
            "An AI agent is downloaded ready to use from a catalogue and starts in an "
            "instant inside a sealed cell. It works with your own model. It can be "
            "paused and resumed like saving a game, and cloned into branches that try "
            "different ideas in parallel, keeping the best one.")
    # The sealed area.
    s.rect(206, 24, 570, 400, t["bg"], t["cellline"], rx=30, dash=True, sw=2)
    badge(s, 236, 56, 17, "shield", t["cellline"], t["bg"])
    s.text(262, 52, "Sealed off", 17, weight=700, fill=t["accent"])
    s.text(262, 71, "nothing gets in or out unless you allow it", 13, fill=t["muted"])

    # Catalogue (left, top) and your model (left, bottom): each reaches the
    # agent in a straight line, square to both boxes and to the sealed edge.
    s.rect(24, 60, 158, 156, t["box"], t["boxline"], rx=16)
    for i, (dx, dy) in enumerate([(-30, 0), (0, -6), (30, 0)]):
        icon(s, "cube", 103 + dx, 106 + dy, 30, t["blue"] if i == 1 else t["muted"])
    s.text(103, 156, "Ready-made", 16, weight=700, anchor="middle")
    s.text(103, 176, "agents", 16, weight=700, anchor="middle")
    s.text(103, 197, "download and go", 13, fill=t["muted"], anchor="middle")
    s.line(182, 198, 266, 198, color=t["blue"], sw=2.2)

    s.rect(24, 244, 158, 156, t["box"], t["boxline"], rx=16)
    icon(s, "chip", 103, 290, 40, t["blue"])
    s.text(103, 342, "Your own model", 16, weight=700, anchor="middle")
    s.text(103, 362, "Ollama, llama.cpp,", 13, fill=t["muted"], anchor="middle")
    s.text(103, 380, "any API", 13, fill=t["muted"], anchor="middle")
    s.line(182, 262, 266, 262, color=t["blue"], sw=2, dash=True)

    # The agent.
    s.rect(266, 178, 136, 104, t["cell"], t["cellline"], rx=18, sw=2.5)
    icon(s, "robot", 334, 216, 42, t["accent"], fill=t["bg"])
    s.text(334, 266, "Your agent", 15, weight=700, anchor="middle")
    badge(s, 398, 182, 16, "bolt", t["gold"], t["bg"])
    s.text(334, 132, "Starts in an instant", 15, weight=700, anchor="middle")
    s.text(334, 151, "microseconds, not seconds", 13, fill=t["muted"], anchor="middle")

    # Fork into branches; keep the best.
    ys = [96, 199, 302]
    for i, y in enumerate(ys):
        s.path(f"M402 230 C 480 230, 520 {y + 31}, 556 {y + 31} H 592",
               color=t["gold"] if i == 0 else t["cellline"], sw=2.5 if i == 0 else 2)
    s.text(496, 98, "Clone it", 15, weight=700, anchor="middle")
    s.text(496, 117, "try ideas in parallel", 13, fill=t["muted"], anchor="middle")
    for i, y in enumerate(ys):
        best = i == 0
        s.rect(592, y, 172, 62, t["cell"] if best else t["box"],
               t["gold"] if best else t["pending"], rx=22, sw=2.5 if best else 1.5)
        icon(s, "robot", 622, y + 31, 26, t["accent"] if best else t["pending"])
        s.text(644, y + 26, ["Branch A", "Branch B", "Branch C"][i], 14, weight=700,
               fill=t["text"] if best else t["muted"])
        s.text(644, y + 45, "the best: keep it" if best else "drop it", 12.5,
               fill=t["gold"] if best else t["muted"], weight=700 if best else 400)
        if best:
            badge(s, 760, y + 4, 13, "star", t["gold"], t["bg"])
        else:
            icon(s, "cross", 744, y + 31, 14, t["pending"])

    # Save slots, like a game.
    s.text(230, 326, "Pause and resume, like saving a game", 15, weight=700)
    for i in range(3):
        x = 230 + i * 108
        last = i == 2
        s.rect(x, 340, 100, 64, t["bg"], t["cellline"] if last else t["boxline"], rx=10,
               sw=2 if last else 1.5)
        icon(s, "save", x + 24, 372, 22, t["accent"] if last else t["muted"])
        s.text(x + 44, 368, f"Save {i + 1}", 13, weight=700)
        s.text(x + 44, 386, ["turn 2", "turn 5", "now"][i], 12, fill=t["muted"])
    return s


# ---------------------------------------------------------------- scenarios
SW, SH = 360, 190


def scene_dev(t):
    s = Svg(t, SH, "A developer tries ten strategies at once",
            "A laptop forks one agent into ten copies that each try a different "
            "strategy; one of them succeeds and is kept.", w=SW)
    icon(s, "laptop", 62, 95, 64, t["text"])
    icon(s, "robot", 62, 90, 22, t["accent"])
    for i in range(10):
        y = 22 + i * 16.2
        win = i == 6
        s.path(f"M100 95 C 160 95, 170 {y}, 236 {y}",
               color=t["gold"] if win else t["cellline"], arrow=False,
               sw=2.4 if win else 1.4)
        s.rect(238, y - 6, 52, 12, t["cell"] if win else t["box"],
               t["gold"] if win else t["cellline"], rx=6, sw=1.6 if win else 1)
    badge(s, 312, 22 + 6 * 16.2, 12, "star", t["gold"], t["bg"])
    s.text(330, 172, "×10", 13, weight=700, fill=t["muted"], anchor="end")
    return s


def scene_company(t):
    s = Svg(t, SH, "A company runs thousands of isolated agents on little hardware",
            "One server holds a grid of hundreds of small cells; most of them sleep "
            "and cost almost nothing, a few are awake and working.", w=SW)
    icon(s, "server", 52, 95, 56, t["text"])
    awake = {3, 17, 22, 40, 51, 58, 66, 79, 85, 92}
    k = 0
    for r in range(7):
        for c in range(14):
            x, y = 104 + c * 17.5, 30 + r * 18
            on = k in awake
            s.rect(x, y, 12, 12, t["cellline"] if on else t["bg"],
                   t["cellline"] if on else t["boxline"], rx=4, sw=1.2)
            k += 1
    s.text(104, 178, "awake", 12, fill=t["accent"], weight=700)
    s.text(150, 178, "asleep: almost free", 12, fill=t["muted"])
    return s


def scene_hobby(t):
    s = Svg(t, SH, "A hobbyist runs a local assistant",
            "At home, a laptop runs a local model next to an agent in a cell; "
            "the agent is suspended when not in use and wakes in an instant.", w=SW)
    icon(s, "home", 52, 60, 40, t["muted"])
    s.rect(110, 40, 108, 108, t["cell"], t["cellline"], rx=30, sw=2)
    icon(s, "robot", 164, 84, 44, t["accent"], fill=t["bg"])
    s.text(164, 132, "assistant", 13, weight=700, anchor="middle")
    s.path("M218 94 L 248 94", color=t["blue"], dash=True, arrow=False, sw=2)
    s.rect(248, 50, 100, 90, t["bluefill"], t["blueline"], rx=14)
    icon(s, "chip", 298, 84, 34, t["blue"])
    s.text(298, 124, "local model", 12, anchor="middle", fill=t["blue"], weight=700)
    icon(s, "pause", 136, 172, 14, t["muted"])
    s.text(148, 177, "sleeps when idle, wakes at once", 12, fill=t["muted"])
    return s


def scene_research(t):
    s = Svg(t, SH, "A researcher freezes and replays an experiment",
            "A timeline of an experiment with saved points; the researcher goes "
            "back to a saved point and replays the run from it exactly.", w=SW)
    icon(s, "flask", 46, 60, 40, t["text"])
    s.line(36, 120, 330, 120, color=t["boxline"], arrow=False, sw=2)
    for i, x in enumerate([100, 170, 240, 300]):
        s.circle(x, 120, 7, t["cellline"] if i != 1 else t["gold"])
    icon(s, "flag", 150, 98, 20, t["gold"])
    s.text(170, 152, "frozen", 12, weight=700, fill=t["gold"], anchor="middle")
    s.path("M300 113 C 300 56, 170 56, 170 88 V 113", color=t["accent"], sw=2)
    s.text(238, 52, "replay from here", 12, weight=700, fill=t["accent"], anchor="middle")
    s.text(330, 176, "same state, every time", 12, fill=t["muted"], anchor="end")
    return s


# ---------------------------------------------------------------- compare
def compare(t):
    s = Svg(t, 506, "A cell, a container and a microVM",
            "Three ways to give an agent its own space. A container shares the host's "
            "Linux kernel with every other container; pausing and cloning are added from "
            "outside. A microVM gives each agent a virtual machine with its own guest "
            "Linux kernel: strong walls, but a whole operating system to boot and keep "
            "per agent. A Rhizo cell runs on one Rhizo kernel inside one VM, many cells "
            "side by side, each in its own address space; create, pause, resume and "
            "clone are operations of that kernel.")
    heading(s, "Three ways to give an agent a room of its own")
    cols = [(24, "Container"), (282, "MicroVM"), (540, "Rhizo cell")]
    cw = 236
    base = 336

    def layer(x, y, w, label, kind="box", h=42, size=13):
        fill, line = {"box": (t["box"], t["boxline"]), "hw": (t["bg"], t["boxline"]),
                      "risk": (t["redfill"], t["redline"]),
                      "rhizo": (t["cell"], t["cellline"]),
                      "agent": (t["bluefill"], t["blueline"])}[kind]
        s.rect(x, y, w, h, fill, line, rx=8, sw=2 if kind == "rhizo" else 1.3)
        if label:
            s.text(x + w / 2, y + h / 2 + 4.5, label, size, anchor="middle",
                   weight=700 if kind in ("rhizo", "agent") else 400)

    def row(n):
        """Top of layer row n above the hardware (row 0)."""
        return base - 48 * n

    for x, name in cols:
        s.text(x + cw / 2, 92, name, 18, weight=700, anchor="middle",
               fill=t["accent"] if name == "Rhizo cell" else t["text"])
        layer(x, base, cw, "Hardware", "hw")
    subs = ["light, but one kernel for all", "a whole OS per agent",
            "many cells, one kernel built for them"]
    for (x, _), sub in zip(cols, subs):
        s.text(x + cw / 2, 116, sub, 12.5, fill=t["muted"], anchor="middle")
    # Container.
    x = 24
    layer(x, row(2), cw, "", "risk", h=90)
    s.text(x + cw / 2, row(2) + 40, "Host Linux kernel", 14, weight=700, anchor="middle")
    s.text(x + cw / 2, row(2) + 62, "shared by every container", 12.5, anchor="middle",
           fill=t["red"], weight=700)
    layer(x, row(3), cw, "Container runtime")
    for i in range(2):
        layer(x + i * 122, row(4) - 6, 114, "agent", "agent", h=48)
    # MicroVM.
    x = 282
    layer(x, row(1), cw, "Host Linux + KVM")
    for i in range(2):
        xx = x + i * 122
        layer(xx, row(2), 114, "VMM")
        layer(xx, row(3), 114, "Guest Linux")
        layer(xx, row(4) - 6, 114, "agent", "agent", h=48)
    # Rhizo.
    x = 540
    layer(x, row(1), cw, "Host + one VMM")
    layer(x, row(3), cw, "", "rhizo", h=90)
    s.text(x + cw / 2, row(3) + 40, "Rhizo kernel", 15, weight=700, anchor="middle")
    s.text(x + cw / 2, row(3) + 62, "create · pause · resume · clone", 12.5,
           anchor="middle", fill=t["accent"], weight=700)
    for i in range(4):
        layer(x + i * 61, row(4) - 6, 53, "cell", "agent", h=48, size=12.5)

    notes = [
        [("cross", "walls: one shared kernel"), ("check", "starts fast"),
         ("cross", "pause, clone: added outside")],
        [("check", "walls: a kernel per agent"), ("cross", "boots a whole OS"),
         ("cross", "pause, clone: whole VM copies")],
        [("check", "walls: own address spaces"), ("check", "starts in microseconds"),
         ("check", "pause, clone: built in")],
    ]
    for (x, _), rows in zip(cols, notes):
        for j, (kind, txt) in enumerate(rows):
            y = 404 + j * 26
            good = kind == "check"
            icon(s, kind, x + 10, y - 5, 15, t["accent"] if good else t["red"], sw=2.6)
            s.text(x + 26, y, txt, 13.5, fill=t["text"] if good else t["muted"])
    s.text(24, 488, "Containers and microVMs can be paused and copied too, with tools added "
           "around them (CRIU, VM snapshots).", 12.5, fill=t["muted"])
    return s


# ---------------------------------------------------------------- story
STEPS = [
    ("spark", "Create", ["an unmodified Linux", "program becomes a cell"]),
    ("play", "Run", ["threads, files,", "network, every CPU"]),
    ("pause", "Suspend", ["its whole state", "frozen in memory"]),
    ("resume", "Resume", ["it carries on", "where it stopped"]),
    ("fork", "Fork", ["copies share memory", "until they write"]),
    ("snow", "Freeze as template", ["warm it up once,", "start many from it"]),
    ("save", "Save to a file", ["a checked file", "you can copy"]),
    ("download", "Load elsewhere", ["another machine", "carries on"]),
]


def story(t):
    s = Svg(t, 450, "The life of a cell",
            "A cell's life in eight steps: " + "; ".join(
                f"{i + 1}. {n}: {' '.join(c)}" for i, (_, n, c) in enumerate(STEPS)) + ".")
    heading(s, "The life of a cell", "Every step is a native operation of the kernel.")
    cw, ch = 168, 146
    xs = [24, 218, 412, 606]
    rows = [92, 280]
    for i, (ic, name, cap) in enumerate(STEPS):
        x, y = xs[i % 4], rows[i // 4]
        other = i == 7
        s.rect(x, y, cw, ch, t["bluefill"] if other else t["cell"],
               t["blueline"] if other else t["cellline"], rx=16, sw=2)
        c = t["blue"] if other else t["accent"]
        s.circle(x + 22, y + 22, 12, c)
        s.text(x + 22, y + 27, str(i + 1), 13, weight=700, fill=t["bg"], anchor="middle")
        s.circle(x + cw / 2, y + 42, 22, t["bg"], c, sw=2)
        icon(s, ic, x + cw / 2, y + 42, 25, c)
        s.text(x + cw / 2, y + 87, name, 15.5, weight=700, anchor="middle")
        for j, ln in enumerate(cap):
            s.text(x + cw / 2, y + 108 + j * 17, ln, 12.5, fill=t["muted"], anchor="middle")
        if i % 4 != 3:
            s.line(x + cw + 1, y + ch / 2, x + 194, y + ch / 2, sw=2)
    # Row 1 to row 2.
    s.path(f"M{606 + cw / 2} {92 + ch} V 259 H {24 + cw / 2} V 280", sw=2)
    return s


# ---------------------------------------------------------------- memory
def memory(t):
    s = Svg(t, 470, "Memory: shared until written",
            "A template holds its pages once. Three forks start from it and share every "
            "page; when a fork writes a page, it gets its own copy of that page only. "
            "Snapshots of suspended forks store only their own pages and refer to the "
            "shared ones, so fifty agents of one template cost little more than one.")
    heading(s, "Memory: shared until written",
            "Forks and snapshots only pay for the pages they change.")
    n, ps, gap = 6, 20, 5

    def strip(x, y, kinds):
        for i, k in enumerate(kinds):
            xx = x + i * (ps + gap)
            if k == "own":
                s.rect(xx, y, ps, ps, t["cellline"], t["cellline"], rx=4, sw=1.2)
            elif k == "shared":
                s.rect(xx, y, ps, ps, t["cell"], t["cellline"], rx=4, sw=1.2, dash=True)
            elif k == "copy":
                # Hatched as well as coloured, so it never relies on colour alone.
                s.rect(xx, y, ps, ps, t["copy"], t["copy"], rx=4, sw=1.2)
                s.path(f"M{xx + 5} {y + ps - 5} L {xx + ps - 5} {y + 5}", color=t["bg"],
                       arrow=False, sw=2)
            else:
                s.rect(xx, y, ps, ps, "none", t["pending"], rx=4, sw=1.2, dash=True)

    # Template.
    tx, ty = 24, 186
    s.rect(tx, ty, 196, 116, t["cell"], t["cellline"], rx=18, sw=2.5)
    icon(s, "snow", tx + 26, ty + 28, 20, t["accent"])
    s.text(tx + 46, ty + 33, "Template", 16, weight=700)
    s.text(tx + 18, ty + 56, "warmed up once", 13, fill=t["muted"])
    strip(tx + 25, ty + 76, ["own"] * n)
    # Forks.
    forks = [("Fork 1", "running", [1]), ("Fork 2", "running", [3, 4]),
             ("Fork 3", "suspended", [0])]
    fx = 316
    fys = [96, 216, 336]
    for (name, state, writes), fy in zip(forks, fys):
        s.path(f"M{tx + 196} {ty + 58} C 252 {ty + 58}, 258 {fy + 44}, 288 {fy + 44} H {fx}")
        s.rect(fx, fy, 196, 88, t["bg"], t["cellline"], rx=18, sw=2)
        s.text(fx + 18, fy + 28, name, 15, weight=700)
        s.text(fx + 178, fy + 28, state, 12.5, anchor="end",
               fill=t["accent"] if state == "running" else t["muted"], weight=700)
        strip(fx + 25, fy + 48, ["copy" if i in writes else "shared" for i in range(n)])
    # Snapshot of the suspended fork.
    sx, sy = 596, 316
    s.line(fx + 196, fys[2] + 44, sx, fys[2] + 44)
    s.text(554, 362, "suspend", 12.5, fill=t["muted"], weight=700, anchor="middle")
    s.rect(sx, sy, 180, 136, t["box"], t["boxline"], rx=14)
    icon(s, "file", sx + 24, sy + 26, 22, t["text"])
    s.text(sx + 44, sy + 32, "Snapshot", 15, weight=700)
    strip(sx + 16, sy + 52, ["copy"] + ["ref"] * 5)
    s.text(sx + 16, sy + 96, "keeps its own page;", 12.5, fill=t["muted"])
    s.text(sx + 16, sy + 114, "points at the others", 12.5, fill=t["muted"])
    # Legend.
    ly = 104
    for i, (k, lab) in enumerate([("own", "a page in memory, once"),
                                  ("shared", "shared: no extra memory"),
                                  ("copy", "copied on its first write"),
                                  ("ref", "a reference, not a copy")]):
        y = ly + i * 34
        strip(596, y, [k])
        s.text(626, y + 15, lab, 13)
    s.text(596, 88, "Pages", 14, weight=700, fill=t["muted"])
    return s


# ---------------------------------------------------------------- architecture
def arch(t):
    s = Svg(t, 600, "Architecture of Rhizo",
            "On the host machine, the rhizo command-line tool talks to a small server, "
            "which drives one virtual machine over its serial line. The VMM (QEMU, "
            "Firecracker or Cloud Hypervisor) runs the Rhizo kernel; above it run the "
            "cells, unmodified Linux programs. The kernel is a set of Rust crates: the "
            "control loop, Linux system calls, sockets, files, memory, the scheduler, "
            "cells and snapshots, over narrow hardware interfaces for AArch64 and x86_64 "
            "and virtio devices. Cells reach the model through the virtual network card.")
    heading(s, "How it fits together")
    mono = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
    s.rect(24, 80, 752, 504, t["bg"], t["boxline"], rx=16)
    s.text(40, 104, "Host machine (Linux with KVM)", 13.5, weight=700, fill=t["muted"])
    # Host programs.
    for x, w, name, sub, blue in [(40, 160, "rhizo CLI", "up · run · fork · chat", False),
                                  (224, 170, "rhizo server", "owns the VM", False),
                                  (560, 200, "Your model", "Ollama, llama.cpp…", True)]:
        s.rect(x, 116, w, 62, t["bluefill"] if blue else t["box"],
               t["blueline"] if blue else t["boxline"], rx=10)
        s.text(x + w / 2, 142, name, 14, weight=700, anchor="middle")
        s.text(x + w / 2, 162, sub, 12, anchor="middle", fill=t["muted"])
    s.line(200, 147, 224, 147, sw=1.8)
    # The VM.
    s.rect(40, 200, 720, 368, t["box"], t["boxline"], rx=14)
    s.text(216, 226, "One VM: QEMU, Firecracker or Cloud Hypervisor", 13.5, weight=700,
           fill=t["muted"])
    s.text(64, 260, "cells: Linux", 12.5, fill=t["muted"])
    s.text(64, 277, "programs", 12.5, fill=t["muted"])
    for i, name in enumerate(["agent", "fork", "template", "tools"]):
        x = 290 + i * 106
        s.rect(x, 240, 96, 46, t["bluefill"], t["blueline"], rx=20, sw=1.5)
        s.text(x + 48, 268, name, 13.5, weight=700, anchor="middle")
    # The kernel.
    s.rect(56, 302, 688, 250, t["cell"], t["cellline"], rx=14, sw=2)
    s.text(72, 330, "Rhizo kernel", 15, weight=700)
    s.text(728, 330, "Rust, no_std, MIT", 12.5, fill=t["muted"], anchor="end")
    blocks = [
        ("Control loop", "rhizo-control"), ("Linux system calls", "rhizo-abi"),
        ("Sockets", "rhizo-net"), ("Files", "rhizo-fs"),
        ("Memory", "rhizo-mm"), ("Scheduler", "rhizo-sched"),
        ("Cells, ELF, quotas", "rhizo-cell"), ("Snapshots", "rhizo-snapshot"),
    ]
    bw, bh = 160, 62
    for i, (name, crate) in enumerate(blocks):
        x = 72 + (i % 4) * (bw + 8)
        y = 344 + (i // 4) * (bh + 8)
        s.rect(x, y, bw, bh, t["bg"], t["cellline"], rx=8, sw=1.2)
        s.text(x + bw / 2, y + 27, name, 13, weight=700, anchor="middle")
        s.add(f'<text x="{x + bw / 2}" y="{y + 46}" font-size="12" fill="{t["muted"]}" '
              f'text-anchor="middle" font-family="{mono}">{crate}</text>')
    s.rect(72, 484, 656, 62, t["bg"], t["boxline"], rx=8, sw=1.2)
    s.text(400, 509, "Narrow hardware interfaces: rhizo-hal, arch/ (AArch64, x86_64), "
           "virtio", 13, weight=700, anchor="middle")
    s.text(400, 529, "so the core runs and is tested on the host, deterministically", 12,
           anchor="middle", fill=t["muted"])
    # Serial line to the control loop; network card to the sockets.
    s.path("M309 178 V 190 H 200 V 344", dash=True, sw=1.8)
    s.text(208, 262, "serial line", 12, fill=t["muted"], weight=700)
    nx = 72 + 2 * (bw + 8) + bw / 2
    s.path(f"M720 178 V 294 H {nx} V 344", color=t["blue"], sw=2)
    s.text(712, 226, "network card", 12, fill=t["blue"], weight=700, anchor="end")
    return s


# ---------------------------------------------------------------- host operations
def hostops(t):
    s = Svg(t, 624, "Host operations stop only the cell's CPUs",
            "Four CPUs run cells A, B and X over time. When the host suspends cell X: "
            "before, every CPU stopped for the operation; now, only the CPUs running X "
            "stop, and cells A and B keep running.")
    heading(s, "Pausing one cell no longer pauses the others",
            "The host suspends cell X while four CPUs run cells A, B and X.")
    x0, x1 = 112, 760
    op0, op1 = 400, 470
    lanes = [[("A", 112, 760)], [("X", 112, 760)], [("B", 112, 560), ("A", 560, 760)],
             [("X", 112, 760)]]
    cols = {"A": (t["cell"], t["cellline"]), "B": (t["bluefill"], t["blueline"]),
            "X": (t["copyfill"], t["copy"])}

    def panel(y, title, all_stop):
        s.text(24, y, title, 15, weight=700,
               fill=t["muted"] if all_stop else t["accent"])
        for i, segs in enumerate(lanes):
            ly = y + 16 + i * 48
            s.text(x0 - 14, ly + 26, f"CPU {i}", 13, fill=t["muted"], anchor="end")
            for name, a, b in segs:
                f, c = cols[name]
                s.rect(a, ly, b - a, 42, f, c, rx=8, sw=1.3)
                for xx in ([a + 30] if b - a < 300 else [a + 30, b - 40]):
                    if not (op0 - 20 < xx < op1 + 10):
                        s.text(xx, ly + 26, name, 13, weight=700, anchor="middle")
            stop = all_stop or any(n == "X" for n, _, _ in segs)
            if stop:
                s.rect(op0, ly, op1 - op0, 42, t["bg"], t["red"], rx=6, sw=1.5,
                       dash=True)
                icon(s, "pause", (op0 + op1) / 2, ly + 21, 16, t["red"])
        return y + 16 + 4 * 48

    icon(s, "pause", op0 - 2, 96, 14, t["red"])
    s.text(op0 + 10, 101, "suspend X", 13, weight=700, fill=t["red"])
    end = panel(124, "Before: every CPU stops", True)
    s.text(x1, 124, "A and B wait too", 12.5, fill=t["red"], anchor="end",
           weight=700)
    end = panel(end + 34, "Now: only the CPUs running X stop", False)
    s.text(x1, end - 4 * 48 - 16, "A and B keep running", 12.5, fill=t["accent"],
           anchor="end", weight=700)
    s.line(x0, end + 16, x1, end + 16, sw=1.5, arrow=False)
    s.text(x1, end + 36, "time →", 12.5, fill=t["muted"], anchor="end")
    return s


# ---------------------------------------------------------------- snapshot path
def snapshot_path(t):
    s = Svg(t, 490, "Saving a cell to a file and loading it elsewhere",
            "On machine A, rhizo save sends a request with a one-time token on the serial "
            "line; the kernel streams the snapshot's bytes over TCP through the VM's "
            "network card to a listener rhizo opened, which writes a file ending in a "
            "checksum. On machine B, rhizo load asks the kernel to fetch the file the same "
            "way; the kernel checks the header, the checksum and every field, caps the "
            "cell's quotas, and then resumes the cell or forks copies of the template.")
    heading(s, "A cell travels as a file", "rhizo save on one machine, rhizo load on another.")
    mono = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"

    def machine(x, title, cmd, cell, note, saving):
        s.rect(x, 84, 300, 332, t["bg"], t["boxline"], rx=16)
        s.text(x + 16, 108, title, 13.5, weight=700, fill=t["muted"])
        s.rect(x + 16, 120, 268, 44, t["box"], t["boxline"], rx=10)
        s.add(f'<text x="{x + 150}" y="{147}" font-size="13" fill="{t["text"]}" '
              f'text-anchor="middle" font-weight="700" font-family="{mono}">{cmd}</text>')
        s.rect(x + 16, 214, 268, 186, t["cell"], t["cellline"], rx=12, sw=2)
        s.text(x + 32, 240, "Rhizo VM", 13.5, weight=700)
        s.rect(x + 32, 254, 112, 60, t["bluefill"], t["blueline"], rx=20, sw=1.5)
        s.text(x + 88, 289, cell, 13, weight=700, anchor="middle")
        s.rect(x + 168, 254, 100, 60, t["bg"], t["cellline"], rx=8, sw=1.2)
        s.text(x + 218, 279, "network", 12.5, weight=700, anchor="middle")
        s.text(x + 218, 296, "card", 12.5, weight=700, anchor="middle")
        for j, ln in enumerate(note):
            s.text(x + 32, 340 + j * 19, ln, 12.5, fill=t["muted"])
        # The request and its token on the serial line (dashed), the bytes over TCP.
        s.line(x + 120, 164, x + 120, 254, dash=True, sw=1.6)
        s.text(x + 112, 184, "request +", 12, fill=t["muted"], anchor="end")
        s.text(x + 112, 199, "one-time token", 12, fill=t["muted"], anchor="end")
        if saving:
            s.line(x + 218, 254, x + 218, 164, color=t["blue"], sw=2.2)
        else:
            s.line(x + 218, 164, x + 218, 254, color=t["blue"], sw=2.2)
        s.text(x + 230, 184, "bytes", 12, fill=t["blue"], weight=700)
        s.text(x + 230, 199, "over TCP", 12, fill=t["blue"], weight=700)

    machine(24, "Machine A", "rhizo save 7 agent.snap", "cell 7",
            ["suspend, then send the bytes;", "shared pages written in full,",
             "so the file stands alone"], True)
    machine(476, "Machine B", "rhizo load agent.snap", "template",
            ["checks: header, checksum,", "every field; quotas capped;",
             "then resume, or fork copies"], False)
    # The file.
    s.rect(352, 190, 96, 120, t["box"], t["boxline"], rx=12)
    icon(s, "file", 400, 228, 40, t["text"])
    s.text(400, 272, "agent.snap", 12.5, weight=700, anchor="middle")
    s.text(400, 290, "+ CRC-32C", 12, anchor="middle", fill=t["muted"])
    s.line(324, 250, 352, 250, sw=2)
    s.line(448, 250, 476, 250, sw=2)
    s.text(400, 446, "Copy it, keep it, share it: the file is the cell, ready to wake up.",
           14, anchor="middle", weight=700)
    s.text(400, 468, "Same architecture on both sides; a file from elsewhere is refused "
           "with its reason.", 12.5, anchor="middle", fill=t["muted"])
    return s


# ---------------------------------------------------------------- security
def security(t):
    s = Svg(t, 586, "Security: trust boundaries and defences",
            "Everything that reaches the Rhizo kernel from cells, snapshot files and the "
            "network is untrusted: hostile cells, hostile snapshot files and hostile "
            "network frames. The host's control requests are trusted but decoded "
            "defensively; the VMM and hardware are trusted. The kernel defends with "
            "privilege levels and own address spaces, quotas, KPTI and E0PD, SMEP, SMAP "
            "and PAN, write-or-execute pages, and zeroed memory. Three internal reviews "
            "fixed or worked around 23 findings and fuzzers keep them fixed; an external audit is scoped "
            "but not done yet.")
    heading(s, "Who is trusted, and what stands in the way")
    # Untrusted inputs.
    ins = [(24, "Hostile cells", "any code, any call"),
           (284, "Snapshot files", "anyone may write one"),
           (544, "Network frames", "any host on the link")]
    for x, name, sub in ins:
        s.rect(x, 84, 232, 64, t["redfill"], t["redline"], rx=12)
        icon(s, {24: "robot", 284: "file", 544: "globe"}[x], x + 30, 116, 24, t["red"])
        s.text(x + 54, 111, name, 15, weight=700)
        s.text(x + 54, 131, sub, 12.5, fill=t["muted"])
        s.line(x + 116, 148, x + 116, 190, color=t["red"], sw=2)
    s.text(776, 72, "untrusted", 12.5, weight=700, fill=t["red"], anchor="end")
    # Kernel with defences.
    s.rect(24, 190, 752, 206, t["cell"], t["cellline"], rx=16, sw=2.5)
    badge(s, 52, 220, 15, "shield", t["cellline"], t["cell"])
    s.text(76, 226, "Rhizo kernel: every input checked at the boundary", 16, weight=700)
    chips = [("Own address space", "per cell, lower half only"),
             ("Quotas", "memory, CPU, sockets"),
             ("KPTI · E0PD", "Meltdown closed"),
             ("SMEP · SMAP · PAN", "kernel stays out"),
             ("Write or execute", "never both, JIT too"),
             ("Zeroed memory", "nothing leaks on reuse")]
    for i, (a, b) in enumerate(chips):
        x = 44 + (i % 3) * 244
        y = 246 + (i // 3) * 70
        s.rect(x, y, 228, 62, t["bg"], t["cellline"], rx=10, sw=1.2)
        icon(s, "check", x + 22, y + 31, 18, t["accent"], sw=2.6)
        s.text(x + 42, y + 27, a, 14, weight=700)
        s.text(x + 42, y + 46, b, 12.5, fill=t["muted"])
    # Trusted below.
    s.line(209, 396, 209, 416, color=t["blue"], sw=2, arrow=False)
    s.rect(24, 416, 370, 62, t["bluefill"], t["blueline"], rx=12)
    s.text(44, 442, "Host control requests", 14, weight=700)
    s.text(44, 462, "trusted, but decoded defensively", 12.5, fill=t["muted"])
    s.rect(406, 416, 370, 62, t["box"], t["boxline"], rx=12)
    s.text(426, 442, "VMM, firmware, hardware", 14, weight=700)
    s.text(426, 462, "trusted, out of scope", 12.5, fill=t["muted"])
    # Assurance.
    s.line(24, 496, 776, 496, color=t["boxline"], arrow=False, sw=1)
    items = [("check", "3 internal reviews", "22 findings fixed, 1 worked around", t["accent"]),
             ("check", "Fuzzers, hostile cells", "calls, snapshots, frames", t["accent"]),
             ("eye", "External audit", "scoped, not done yet", t["warn"])]
    for i, (ic, a, b, c) in enumerate(items):
        x = 24 + i * 254
        icon(s, ic, x + 16, 522, 20, c, sw=2.4)
        s.text(x + 36, 519, a, 14, weight=700, fill=c if ic == "eye" else t["text"])
        s.text(x + 36, 538, b, 12.5, fill=t["muted"])
    s.text(24, 568, "Details: docs/security.md (threat model, findings), "
           "docs/audit-scope.md (what an auditor gets).", 12.5, fill=t["muted"])
    return s


# ---------------------------------------------------------------- hierarchy
def hierarchy(t):
    s = Svg(t, 560, "Designed: the cell hierarchy",
            "Designed, not built yet. The host is the root; each of its children roots a "
            "domain. Agent A, with egress to the model only and a budget of 1 GiB and 8 "
            "cells, has two children with no egress: tools, reached over a channel, and "
            "a python module. Agent B has an allowlist egress policy and a worker child. "
            "Authority flows down. Between domains nothing is shared: no loopback, no "
            "files, no counters; only slow timing channels remain, measured. Handles carry "
            "rights that can only shrink.")
    heading(s, "Next: agents in families, walled from each other")
    tagline(s, 776, 20, "designed, not built yet")
    # Host.
    s.rect(330, 74, 140, 42, t["box"], t["boxline"], rx=21)
    icon(s, "user", 362, 95, 18, t["text"])
    s.text(410, 100, "host", 14, weight=700, anchor="middle")
    # Domains.
    s.rect(24, 140, 444, 304, t["bg"], t["cellline"], rx=18, dash=True, sw=1.8)
    s.text(40, 166, "Domain 1", 13, weight=700, fill=t["accent"])
    s.rect(500, 140, 276, 304, t["bg"], t["cellline"], rx=18, dash=True, sw=1.8)
    s.text(760, 166, "Domain 2", 13, weight=700, fill=t["accent"], anchor="end")
    # Square elbows: each edge crosses a domain's border at a right angle.
    s.path("M360 116 V 122 Q 360 128 354 128 H 242 Q 236 128 236 134 V 180")
    s.path("M440 116 V 122 Q 440 128 446 128 H 632 Q 638 128 638 134 V 180")

    def node(x, y, w, name, egress, budget=None, mod=False):
        s.rect(x, y, w, 86 if budget else 68, t["cell"], t["cellline"], rx=16, sw=2)
        icon(s, "cube" if mod else "robot", x + 26, y + 25, 22, t["accent"])
        s.text(x + 46, y + 30, name, 14.5, weight=700)
        none = egress == "none"
        icon(s, "globe", x + 24, y + 48, 15, t["muted"] if none else t["blue"], sw=1.8)
        s.text(x + 38, y + 52, f"egress: {egress}", 12.5,
               fill=t["muted"] if none else t["blue"], weight=700)
        if budget:
            s.text(x + 16, y + 71, budget, 12, fill=t["muted"])

    node(152, 180, 168, "Agent A", "model", "1 GiB · 8 cells")
    node(44, 330, 168, "tools", "none")
    node(256, 330, 188, "python", "none", mod=True)
    node(554, 180, 168, "Agent B", "allowlist", "512 MiB · 4 cells")
    node(554, 330, 168, "worker", "none")
    # Tree edges; the two children of A are also reached over a channel.
    s.path("M200 266 V 292 Q 200 298 194 298 H 134 Q 128 298 128 304 V 330")
    s.path("M272 266 V 292 Q 272 298 278 298 H 344 Q 350 298 350 304 V 330")
    s.path("M638 266 V 330")
    s.text(124, 292, "+ channel", 12, fill=t["gold"], weight=700, anchor="end")
    s.text(358, 292, "+ channel", 12, fill=t["gold"], weight=700)
    s.text(246, 426, "authority flows down: a cell acts only on its descendants", 12.5,
           fill=t["muted"], anchor="middle")
    s.text(638, 426, "budget carved from its parent", 12.5, fill=t["muted"],
           anchor="middle")
    # The wall.
    s.rect(474, 196, 20, 200, t["redfill"], t["redline"], rx=6, sw=1.2)
    icon(s, "cross", 484, 296, 14, t["red"], sw=2.6)
    # Legend.
    leg = [("No shared loopback, files or counters", "storage channels closed"),
           ("Egress checked on every socket", "inherited, only narrowed; default: none"),
           ("Handles with rights that only shrink", "cells, channels, views; revocable")]
    for i, (a, b) in enumerate(leg):
        x = 24 + i * 254
        s.text(x, 474, a, 13, weight=700)
        s.text(x, 493, b, 12, fill=t["muted"])
    s.text(24, 536, "Timing channels on shared CPUs remain: listed and measured, and reduced "
           "further with exclusive CPU sets.", 12.5, fill=t["muted"])
    return s


# ---------------------------------------------------------------- modules
def modules(t):
    s = Svg(t, 560, "Designed: modular cells",
            "Designed, not built yet. Supply: a builder cell in its own domain installs "
            "packages only through rhizo proxy on the host, which serves only the files "
            "the lockfile names, checks each pinned SHA-256, takes a token per build and "
            "logs every request; the operator reviews and signs the result into a signed "
            "catalogue of templates. Use: templates form a tree that shares memory (python, "
            "python plus numpy, plus pandas; node). An agent asks to add numpy; the kernel "
            "checks the whitelist, budget and egress, asks the parent for sensitive "
            "modules, and forks a child from the warm template, reached over a channel. "
            "Adding a module never grants network access.")
    heading(s, "Next: abilities as child cells, from a signed catalogue")
    tagline(s, 776, 20, "designed, not built yet")
    s.text(24, 104, "Building a module", 14, weight=700, fill=t["muted"])
    boxes = [("globe", "Packages", ["from PyPI, over", "verified TLS"], "box"),
             ("lock", "rhizo proxy", ["lockfile files only,", "SHA-256 checked"], "box"),
             ("cube", "Builder cell", ["own domain,", "same tree twice"], "cell"),
             ("key", "Operator", ["reviews and signs:", "minisign, Ed25519"], "box"),
             ("shield", "Catalogue", ["signed snapshot", "files + manifest"], "cell")]
    bw, step = 140, 152
    for i, (ic, name, lines, kind) in enumerate(boxes):
        x = 24 + i * step
        cell = kind == "cell"
        s.rect(x, 116, bw, 90, t["cell"] if cell else t["box"],
               t["cellline"] if cell else t["boxline"], rx=12, sw=2 if cell else 1.3)
        icon(s, ic, x + 24, 141, 20, t["accent"] if cell else t["text"])
        s.text(x + 42, 146, name, 13.5, weight=700)
        for j, ln in enumerate(lines):
            s.text(x + 14, 172 + j * 18, ln, 12, fill=t["muted"])
        if i < len(boxes) - 1:
            s.line(x + bw, 161, x + step, 161, sw=2)
    s.line(24, 226, 776, 226, color=t["boxline"], arrow=False, sw=1)
    s.text(24, 254, "Using a module: templates from the catalogue, then a child cell", 14,
           weight=700, fill=t["muted"])
    # Template tree.
    for x, y, w, name in [(24, 270, 170, "python"), (24, 336, 170, "python + numpy"),
                          (40, 402, 170, "+ pandas"), (210, 270, 120, "node")]:
        s.rect(x, y, w, 42, t["cell"], t["cellline"], rx=14, sw=1.8)
        icon(s, "snow", x + 22, y + 21, 16, t["accent"])
        s.text(x + 38, y + 26, name, 13.5, weight=700)
    s.line(44, 312, 44, 336, sw=2)
    s.line(60, 378, 60, 402, sw=2)
    s.text(24, 470, "templates share memory", 12.5, fill=t["muted"])
    s.text(24, 488, "copy-on-write, delta files", 12.5, fill=t["muted"])
    # Agent, kernel checks, child cell.
    s.rect(380, 280, 150, 90, t["cell"], t["cellline"], rx=18, sw=2)
    icon(s, "robot", 404, 304, 22, t["accent"])
    s.text(424, 310, "Agent", 14.5, weight=700)
    s.text(396, 334, "static chat agent", 12, fill=t["muted"])
    s.text(396, 352, "asks: add numpy", 12, fill=t["muted"])
    s.line(530, 320, 590, 320, sw=2)
    s.rect(590, 270, 186, 120, t["box"], t["boxline"], rx=12)
    s.text(606, 298, "Kernel checks", 13.5, weight=700)
    for j, ln in enumerate(["on the whitelist?", "within the budget?",
                            "sensitive? ask parent", "egress: never more"]):
        icon(s, "check", 614, 316 + j * 18, 12, t["accent"], sw=2.6)
        s.text(626, 320 + j * 18, ln, 12, fill=t["muted"])
    s.line(683, 390, 683, 420, sw=2)
    s.rect(590, 420, 186, 62, t["cell"], t["cellline"], rx=18, sw=2)
    icon(s, "cube", 614, 451, 20, t["accent"])
    s.text(632, 447, "numpy child cell", 13.5, weight=700)
    s.text(632, 466, "fork of the template", 12, fill=t["muted"])
    s.path("M590 451 H 455 V 370", color=t["gold"], dash=True, arrow=False, sw=2.2)
    s.text(466, 440, "channel", 12, fill=t["gold"], weight=700)
    s.path("M194 357 C 280 357, 300 470, 400 470 H 590", dash=True)
    s.text(250, 520, "Adding requests grants no internet:", 12.5, fill=t["muted"])
    s.text(250, 538, "a module never gets more egress than its agent.", 12.5, fill=t["muted"])
    return s


for name, fn in [("hero", hero), ("scene-dev", scene_dev), ("scene-company", scene_company),
                 ("scene-hobby", scene_hobby), ("scene-research", scene_research),
                 ("compare", compare), ("story", story), ("memory", memory),
                 ("architecture", arch), ("hostops", hostops),
                 ("snapshot-path", snapshot_path), ("security", security),
                 ("hierarchy", hierarchy), ("modules", modules)]:
    both(name, fn)
print("ok")
