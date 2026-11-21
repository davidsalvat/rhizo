#!/usr/bin/env node
// Layout and accessibility checks for the README's SVGs (light and dark).
//
//     node docs/img/check.js [--shots DIR] [file.svg ...]
//
// Without files it checks every SVG in docs/img/ and bench/. It renders each
// one in headless Chromium (Playwright), measures every element with the
// browser (getBBox, getBoundingClientRect, getPointAtLength) and fails on:
//
// - an element outside the viewBox or closer than 8 units to its edge;
// - text with less than 12 units of padding inside the box that holds it, or
//   touching a box it is not inside, or overlapping other text, an icon, a
//   dot or a line;
// - text under 12 units (the images are 800 units wide, shown at 800 px);
// - an arrow whose head stops short of its target, enters a box its line
//   does not start in, or repeats another arrow;
// - contrast under 4.5:1 for text (3:1 for large text) and under 3:1 for
//   lines, arrows, icons, bars and box outlines, against what is behind them.
//
// With --shots DIR it also writes a PNG of each file on its page background.
// Needs Playwright (`npm install -g playwright`, or NODE_PATH pointing at it);
// CHROME names a browser binary when Playwright has none of its own.
const fs = require('fs');
const path = require('path');
let playwright;
try { playwright = require('playwright'); } catch (e) {
  playwright = require('/opt/node22/lib/node_modules/playwright');
}

const root = path.resolve(__dirname, '..', '..');
const args = process.argv.slice(2);
let shots = null;
const files = [];
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--shots') shots = args[++i];
  else files.push(path.resolve(args[i]));
}
if (!files.length) {
  for (const dir of ['docs/img', 'bench']) {
    for (const f of fs.readdirSync(path.join(root, dir)).sort()) {
      if (f.endsWith('.svg')) files.push(path.join(root, dir, f));
    }
  }
}

function inPage(pageBack) {
  const PAD = 12, EDGE = 8, TOUCH = 4, MIN_FONT = 12;
  const svg = document.querySelector('svg');
  const vb = svg.viewBox.baseVal;
  const origin = svg.getBoundingClientRect();
  const k = vb.width / origin.width;
  const out = [];
  const box = el => {
    const r = el.getBoundingClientRect();
    return { x: (r.left - origin.left) * k, y: (r.top - origin.top) * k,
             w: r.width * k, h: r.height * k };
  };
  const inside = (p, b, m = 0) => p.x > b.x + m && p.x < b.x + b.w - m &&
                                  p.y > b.y + m && p.y < b.y + b.h - m;
  const meet = (a, b, m = 0) => a.x < b.x + b.w + m && b.x < a.x + a.w + m &&
                                a.y < b.y + b.h + m && b.y < a.y + a.h + m;
  const name = el => (el.textContent || el.tagName).trim().slice(0, 40);

  // Colours and contrast (WCAG 2.1 relative luminance).
  const ctx = document.createElement('canvas').getContext('2d');
  const rgb = c => {
    ctx.fillStyle = '#000'; ctx.fillStyle = c;
    const h = ctx.fillStyle;
    if (h.startsWith('#')) return [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
    return h.match(/[\d.]+/g).slice(0, 3).map(Number);
  };
  const lum = c => {
    const [r, g, b] = rgb(c).map(v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
  const paint = (el, attr) => { const v = el.getAttribute(attr); return v && v !== 'none' ? v : null; };

  // What is drawn, in document order.
  const icons = [...svg.querySelectorAll('g[transform]')];
  const inIcon = el => icons.some(g => g.contains(el));
  const all = [...svg.querySelectorAll('rect, circle, text, line, path, g[transform]')];
  const bg = svg.querySelector('rect.bg');
  const pageBg = bg ? bg.getAttribute('fill') : pageBack;
  const shapes = all.filter(el => (el.tagName === 'rect' || el.tagName === 'circle') && el !== bg && !inIcon(el));
  const rects = shapes.filter(el => el.tagName === 'rect').map(el => ({ el, b: box(el) }));
  const order = el => all.indexOf(el);
  // The colour behind point p, under element `el` (the last filled shape before it).
  const behind = (p, el) => {
    let c = pageBg;
    for (const s of shapes) {
      if (order(s) >= order(el)) break;
      const f = paint(s, 'fill');
      if (f && s.getAttribute('opacity') === null && inside(p, box(s))) c = f;
    }
    return c;
  };

  for (const el of all) {
    if (el === bg) continue;
    const b = box(el);
    if (b.w === 0 && b.h === 0) continue;
    if (b.x < EDGE || b.y < EDGE || b.x + b.w > vb.width - EDGE || b.y + b.h > vb.height - EDGE) {
      if (!inIcon(el) || el.tagName === 'g') out.push(`outside the viewBox margin: <${el.tagName}> "${name(el)}"`);
    }
  }

  // Text.
  const texts = [...svg.querySelectorAll('text')].filter(t => t.textContent.trim());
  const tb = texts.map(t => {
    const r = t.getBBox();
    return { t, b: { x: r.x, y: r.y, w: r.width, h: r.height } };
  });
  const lines = [...svg.querySelectorAll('line, path')].filter(el => !inIcon(el));
  const dots = shapes.filter(el => el.tagName === 'circle').map(el => ({ el, b: box(el) }));
  const iconBoxes = icons.map(g => ({ el: g, b: box(g) }));
  for (const { t, b } of tb) {
    const s = t.textContent.trim().slice(0, 50);
    const style = getComputedStyle(t);
    const size = parseFloat(style.fontSize);
    const weight = parseInt(style.fontWeight);
    if (size < MIN_FONT) out.push(`font under ${MIN_FONT}: "${s}" (${size})`);
    const c = { x: b.x + b.w / 2, y: b.y + b.h / 2 };
    for (const r of rects) {
      if (order(r.el) > order(t)) continue;
      if (inside(c, r.b)) {
        const pad = Math.min(b.x - r.b.x, r.b.x + r.b.w - b.x - b.w, b.y - r.b.y, r.b.y + r.b.h - b.y - b.h);
        if (pad < PAD - 0.5) out.push(`padding ${pad.toFixed(1)} < ${PAD} inside its box: "${s}"`);
      } else if (meet(b, r.b, TOUCH) && !(r.b.w < 4 || r.b.h < 4)) {
        const solid = paint(r.el, 'stroke') || paint(r.el, 'fill');
        if (solid) out.push(`touches a box it is not in: "${s}"`);
      }
    }
    for (const o of tb) if (o.t !== t && order(o.t) > order(t) && meet(b, o.b, -1)) out.push(`text overlaps text: "${s}" / "${o.t.textContent.trim().slice(0, 40)}"`);
    for (const i of iconBoxes.concat(dots)) {
      const onBadge = i.el.tagName === 'circle' && b.x >= i.b.x && b.y >= i.b.y && b.x + b.w <= i.b.x + i.b.w && b.y + b.h <= i.b.y + i.b.h;
      if (meet(b, i.b, 1) && !onBadge) out.push(`text overlaps an icon or dot: "${s}"`);
    }
    for (const l of lines) {
      const n = l.getTotalLength();
      for (let d = 0; d <= n; d += 1.5) {
        const p = l.getPointAtLength(d);
        if (inside(p, b, -2)) { out.push(`a line crosses text: "${s}"`); break; }
      }
    }
    const fill = style.fill;
    const large = size >= 24 || (size >= 18.66 && weight >= 700);
    const ratio = contrast(fill, behind(c, t));
    if (ratio < (large ? 3 : 4.5)) out.push(`text contrast ${ratio.toFixed(2)} < ${large ? 3 : 4.5}: "${s}"`);
  }

  // Arrows.
  const seen = new Map();
  for (const l of lines) {
    const n = l.getTotalLength();
    if (!n) continue;
    const a = l.getPointAtLength(0), e = l.getPointAtLength(n);
    const stroke = paint(l, 'stroke');
    if (stroke) {
      const m = l.getPointAtLength(n / 2);
      const ratio = contrast(stroke, behind(m, l));
      if (ratio < 3) out.push(`line contrast ${ratio.toFixed(2)} < 3 (${stroke})`);
    }
    if (!l.getAttribute('marker-end')) continue;
    const key = [a.x, a.y, e.x, e.y].map(v => Math.round(v)).join(',');
    if (seen.has(key)) out.push(`duplicate arrow at ${key}`);
    seen.set(key, true);
    // The head must sit on the edge of a box or a dot: not short of it, not inside it.
    let nearest = Infinity;
    for (const r of rects.concat(dots)) {
      const q = r.b;
      const d = r.el.tagName === 'circle'
        ? Math.abs(Math.hypot(e.x - q.x - q.w / 2, e.y - q.y - q.h / 2) - q.w / 2)
        : inside(e, q)
        ? Math.min(e.x - q.x, q.x + q.w - e.x, e.y - q.y, q.y + q.h - e.y)
        : Math.hypot(Math.max(q.x - e.x, 0, e.x - q.x - q.w), Math.max(q.y - e.y, 0, e.y - q.y - q.h));
      nearest = Math.min(nearest, d);
    }
    if (nearest > 1.5) out.push(`arrowhead ${nearest.toFixed(1)} away from any box edge at ${key}`);
  }

  // Graphics: icons, bars, dots and box outlines need 3:1 against what is behind.
  for (const g of icons) {
    const c = g.getAttribute('stroke');
    const p = { x: box(g).x + box(g).w / 2, y: box(g).y + box(g).h / 2 };
    // An icon drawn on a badge is measured against the badge.
    const ratio = contrast(c, behind(p, g));
    if (ratio < 3) out.push(`icon contrast ${ratio.toFixed(2)} < 3`);
  }
  for (const s of shapes) {
    const b = box(s);
    const p = { x: b.x + b.w / 2, y: b.y + b.h / 2 };
    const back = behind(p, s);
    const fill = paint(s, 'fill'), stroke = paint(s, 'stroke');
    const sw = parseFloat(s.getAttribute('stroke-width') || '1');
    const edge = stroke && sw > 0 ? contrast(stroke, back) : 0;
    const body = fill ? contrast(fill, back) : 0;
    if (Math.max(edge, body) < 3) out.push(`<${s.tagName}> at ${Math.round(b.x)},${Math.round(b.y)}: outline ${edge.toFixed(2)}, fill ${body.toFixed(2)} < 3`);
  }
  return [...new Set(out)];
}

(async () => {
  const launch = process.env.CHROME ? { executablePath: process.env.CHROME } : {};
  let browser;
  try { browser = await playwright.chromium.launch(launch); } catch (e) {
    browser = await playwright.chromium.launch({ executablePath: '/usr/bin/google-chrome' });
  }
  const page = await browser.newPage({ deviceScaleFactor: 1.5 });
  let failed = 0;
  for (const f of files) {
    const svg = fs.readFileSync(f, 'utf8');
    const dark = f.endsWith('-dark.svg');
    const back = dark ? '#0d1117' : '#ffffff';
    await page.setContent(`<html><body style="margin:0;padding:16px;background:${back}">${svg}</body></html>`);
    await page.evaluate(() => document.fonts.ready);
    const problems = await page.evaluate(inPage, back);
    const rel = path.relative(root, f);
    for (const p of problems) console.log(`${rel}: ${p}`);
    if (problems.length) failed++;
    if (shots) {
      fs.mkdirSync(shots, { recursive: true });
      await (await page.$('svg')).screenshot({ path: path.join(shots, path.basename(f, '.svg') + '.png') });
    }
  }
  await browser.close();
  console.log(`${files.length} images checked, ${failed} with problems`);
  process.exit(failed ? 1 : 0);
})();
