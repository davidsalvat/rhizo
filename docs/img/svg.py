"""Shared drawing style of the README graphics: themes, the `Svg` writer and
small icons. `gen.py` (charts from a benchmark run) and `diagrams.py`
(hand-designed diagrams, no data) both draw with it, so every image has the
same look in light and dark themes."""
from pathlib import Path

OUT = Path(__file__).resolve().parent
FONT = "Inter, -apple-system, Segoe UI, Helvetica, Arial, sans-serif"
W = 800

THEMES = {
    "light": dict(
        bg="#ffffff", border="#d0d7de", text="#0e2a20", muted="#4b5c55",
        box="#f3f5f4", boxline="#858f8a",
        cell="#eafaf2", cellline="#1f9d6b", accent="#146c4a",
        bar="#1f9d6b", other="#77818b", warn="#875a00", warnfill="#fff4d6",
        done="#16784f", donetext="#ffffff", arrow="#57706a", pending="#7d8590",
        blue="#0a5cc2", bluefill="#e3f0ff", blueline="#3f80cf",
        red="#b42318", redfill="#fdecea", redline="#d0604f",
        copy="#c26a00", copyfill="#ffe9cc", gold="#8a6100",
    ),
    "dark": dict(
        bg="#0d1117", border="#30363d", text="#e6edf3", muted="#9ba7b3",
        box="#161b22", boxline="#6e7681",
        cell="#0f2a1f", cellline="#63d4a0", accent="#63d4a0",
        bar="#63d4a0", other="#768390", warn="#e3b341", warnfill="#2e2513",
        done="#63d4a0", donetext="#0e2a20", arrow="#8b98a5", pending="#768390",
        blue="#79b8ff", bluefill="#0f2440", blueline="#3f7dc4",
        red="#ff8a80", redfill="#3a1d1d", redline="#b5524a",
        copy="#f0a650", copyfill="#3a2a12", gold="#f2c14e",
    ),
}


# Arrowheads, one per line colour: `marker=` names them.
MARKERS = {"a": "arrow", "g": "cellline", "b": "blue", "o": "gold", "r": "red"}


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


class Svg:
    def __init__(self, t, h, title, desc, w=W, frame=True):
        self.t, self.h, self.w, self.parts = t, h, w, []
        self.head = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
            f'width="{w}" height="{h}" role="img" font-family="{FONT}">\n'
            f"<title>{esc(title)}</title>\n<desc>{esc(desc)}</desc>\n"
            "<defs>" + "".join(
                f'<marker id="{k}" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="7" '
                f'markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" '
                f'fill="{t[c]}"/></marker>' for k, c in MARKERS.items()) + "</defs>\n"
            + (f'<rect class="bg" x="0.5" y="0.5" width="{w-1}" height="{h-1}" rx="12" '
               f'fill="{t["bg"]}" stroke="{t["border"]}"/>\n' if frame else
               f'<rect class="bg" x="0" y="0" width="{w}" height="{h}" rx="12" '
               f'fill="{t["bg"]}"/>\n')
        )

    def add(self, s):
        self.parts.append(s)

    def rect(self, x, y, w, h, fill, stroke, rx=8, dash=False, sw=1.5, opacity=None):
        d = ' stroke-dasharray="5 4"' if dash else ""
        o = f' opacity="{opacity}"' if opacity is not None else ""
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" '
                 f'fill="{fill}" stroke="{stroke}" stroke-width="{sw}"{d}{o}/>')

    def circle(self, x, y, r, fill, stroke="none", sw=1.5, dash=False):
        d = ' stroke-dasharray="4 3"' if dash else ""
        self.add(f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}" stroke="{stroke}" '
                 f'stroke-width="{sw}"{d}/>')

    def text(self, x, y, s, size=16, fill=None, weight=400, anchor="start", halo=False,
             italic=False):
        fill = fill or self.t["text"]
        a = f' text-anchor="{anchor}"' if anchor != "start" else ""
        w = f' font-weight="{weight}"' if weight != 400 else ""
        i = ' font-style="italic"' if italic else ""
        h = (f' stroke="{self.t["bg"]}" stroke-width="5" stroke-linejoin="round" '
             'paint-order="stroke"') if halo else ""
        self.add(f'<text x="{x}" y="{y}" font-size="{size}" fill="{fill}"{w}{a}{i}{h}>'
                 f'{esc(s)}</text>')

    def line(self, x1, y1, x2, y2, color=None, arrow=True, sw=1.8, dash=False, marker="a"):
        color = color or self.t["arrow"]
        m = f' marker-end="url(#{marker})"' if arrow else ""
        d = ' stroke-dasharray="4 4"' if dash else ""
        self.add(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{color}" '
                 f'stroke-width="{sw}"{m}{d}/>')

    def path(self, d, color=None, arrow=True, sw=1.8, dash=False, fill="none", marker="a"):
        color = color or self.t["arrow"]
        m = f' marker-end="url(#{marker})"' if arrow else ""
        da = ' stroke-dasharray="4 4"' if dash else ""
        self.add(f'<path d="{d}" fill="{fill}" stroke="{color}" stroke-width="{sw}" '
                 f'stroke-linecap="round" stroke-linejoin="round"{m}{da}/>')

    def save(self, name):
        (OUT / name).write_text(self.head + "\n".join(self.parts) + "\n</svg>\n")


def both(name, fn):
    """Writes `name.svg` and `name-dark.svg` from `fn(theme)`."""
    for theme, t in THEMES.items():
        svg = fn(t)
        if svg:
            svg.save(f"{name}{'-dark' if theme == 'dark' else ''}.svg")
