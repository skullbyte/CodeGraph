// Work out once per filter change which files are shown and which match the search.
function refreshVis() {
  shown = []; foundCount = 0;
  for (const n of nodes) {
    n.vis = visible(n);
    n.m = !filters.text || n.hay.includes(filters.text);
    if (n.vis) { shown.push(n); if (n.m) foundCount++; }
  }
  shownEdges = edges.filter(e => nodes[e.a].vis && nodes[e.b].vis);
}
function filtersChanged() {
  refreshVis();
  if (selected >= 0 && !nodes[selected].vis) select(-1);
  invalidate();
}

function readColors() {
  const style = getComputedStyle(document.documentElement), v = name => style.getPropertyValue(name).trim();
  css = {
    dark: darkTheme(), bg: v('--bg'), bgTop: v('--bg-top'), bgBottom: v('--bg-bottom'), fg: v('--fg'),
    edge: v('--edge'), grid: v('--grid'), shadow: v('--shadow'), accent: v('--accent'),
  };
}

function resize() {
  const box = canvas.parentElement.getBoundingClientRect();
  dpr = Math.min(window.devicePixelRatio || 1, 2); // 3x screens don't need 9x the pixels
  canvas.width = bgc.width = Math.round(box.width * dpr); canvas.height = bgc.height = Math.round(box.height * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0); bctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  readColors();
  fitDirty = true;
  if (!userMoved) fit();
  bgSig[3] = -1; // the cleared backdrop canvas must be painted again
  draw();
  paintBg(performance.now());
}

// Camera: rotate around y, then tilt around x, then perspective divide.
const cam = { cy: 1, sy: 0, cx: 1, sx: 0, ox: 0, oy: 0 };
const P = { x: 0, y: 0, z: 0, s: 1 };
function project(x, y, z) {
  const x1 = x * cam.cy + z * cam.sy, z1 = -x * cam.sy + z * cam.cy;
  const y2 = y * cam.cx - z1 * cam.sx, z2 = y * cam.sx + z1 * cam.cx;
  const s = D / Math.max(D + z2, 1);
  P.x = cam.ox + x1 * s * view.k; P.y = cam.oy + y2 * s * view.k; P.z = z2; P.s = s;
  return P;
}
const depthOf = z => clamp((z + R) / (2 * R), 0, 1); // 0 = near, 1 = far

// Intro: balls fade and grow in from 60% size, ease-out, lightly staggered.
const grow = n => {
  const t = clamp((animNow - born - (n.i / Math.max(nodes.length, 1)) * 500) / 420, 0, 1);
  return 1 - Math.pow(1 - t, 3);
};

function projectAll(intro) {
  const w = canvas.clientWidth, h = canvas.clientHeight;
  cam.cy = Math.cos(rot.y); cam.sy = Math.sin(rot.y); cam.cx = Math.cos(rot.x); cam.sx = Math.sin(rot.x);
  cam.ox = w / 2 + view.x; cam.oy = h / 2 + view.y;
  for (const n of shown) {
    if (mode3d) { project(n.x, n.y, n.z); n.px = P.x; n.py = P.y; n.pz = P.z; n.ps = P.s; n.t = depthOf(P.z); }
    else { n.px = cam.ox + n.x * view.k; n.py = cam.oy + n.y * view.k; n.pz = 0; n.ps = 1; n.t = 0; }
    n.g = intro ? grow(n) : 1;
    n.sr = Math.max(1.5, radius(n) * n.ps * view.k) * (0.6 + 0.4 * n.g);
  }
}

// Atmospheric perspective: far balls are veiled toward the background colour.
function veil(n) {
  let f = mode3d ? 0.7 * n.t : 0;
  if (!n.m) f = 0.86;
  else if (selected >= 0 && n.i !== selected && !near.has(n.i)) f = Math.max(f, 0.72);
  return f;
}

// Hubs, the selected file and its neighbours glow (dark theme); in the light theme they get a soft shadow.
const glows = n => n.m && (selected >= 0 ? n.i === selected || (near.has(n.i) && near.size <= 40) : hubSet.has(n.i) && !filters.text);

// A floor grid below the graph: it recedes into the horizon and catches soft shadows.
function drawFloor(list, shadows) {
  const show = clamp(cam.sx / 0.3, 0, 1); // fades out as the view turns edge-on or from below
  if (show <= 0.02) return;
  const F = R * 1.0, E = R * 2.3, N = 12, gap = E / N, SEG = 12, squash = clamp(cam.sx, 0.15, 1);
  if (css.dark) { // a faint pool of light where the floor meets the horizon
    project(0, F, 0);
    const gw = E * 2 * P.s * view.k, sp = horizonGlow();
    ctx.globalCompositeOperation = 'lighter'; ctx.globalAlpha = 0.5 * show;
    ctx.drawImage(sp, P.x - gw / 2, P.y - (gw * squash) / 2, gw, gw * squash);
    ctx.globalCompositeOperation = 'source-over';
  }
  const paths = [new Path2D(), new Path2D(), new Path2D(), new Path2D()];
  for (let pass = 0; pass < 2; pass++) {
    for (let i = -N; i <= N; i++) {
      for (let j = 0; j < SEG; j++) {
        const u0 = -E + (2 * E * j) / SEG, u1 = u0 + (2 * E) / SEG, um = (u0 + u1) / 2, c = i * gap;
        const mx = pass ? um : c, mz = pass ? c : um;
        const edgeFade = 1 - (mx * mx + mz * mz) / (E * E);
        if (edgeFade <= 0) continue;
        const a = Math.pow(edgeFade, 0.8) * (1 - 0.7 * depthOf(project(mx, F, mz).z));
        const b = Math.min(3, Math.floor(a * 4));
        if (b <= 0) continue;
        project(pass ? u0 : c, F, pass ? c : u0); paths[b].moveTo(P.x, P.y);
        project(pass ? u1 : c, F, pass ? c : u1); paths[b].lineTo(P.x, P.y);
      }
    }
  }
  ctx.strokeStyle = css.grid; ctx.lineWidth = 1;
  const base = (css.dark ? 0.5 : 0.34) * show;
  for (let b = 1; b < 4; b++) { ctx.globalAlpha = base * b / 3; ctx.stroke(paths[b]); }
  if (shadows) {
    const sp = shadow();
    for (const n of list) {
      const lift = clamp((F - n.y) / (2 * R), 0, 1); // higher above the floor: wider, fainter shadow
      project(n.x, F, n.z);
      const w = radius(n) * P.s * view.k * 2.6 * (1 + lift * 0.8) * (0.6 + 0.4 * n.g);
      ctx.globalAlpha = (css.dark ? 0.5 : 0.22) * (1 - 0.7 * lift) * (1 - 0.6 * depthOf(P.z)) * show * n.g * (n.m ? 1 : 0.3);
      ctx.drawImage(sp, P.x - w / 2, P.y - (w * squash) / 2, w, w * squash);
    }
  }
  ctx.globalAlpha = 1;
}

function horizonGlow() {
  if (floorGlow) return floorGlow;
  const S = 256, m = S / 2;
  floorGlow = makeCanvas(S);
  const g = floorGlow.getContext('2d'), gr = g.createRadialGradient(m, m, 0, m, m, m);
  for (const t of [0, 0.15, 0.3, 0.45, 0.6, 0.8, 1]) { // smooth falloff: many gentle steps avoid visible rings
    const k = Math.pow(1 - t, 2);
    gr.addColorStop(t, `rgba(${Math.round(60 + 36 * k)},${Math.round(80 + 44 * k)},${Math.round(200 + 55 * k)},${(0.34 * k).toFixed(4)})`);
  }
  g.fillStyle = gr; g.fillRect(0, 0, S, S);
  return floorGlow;
}

function arrow(path, x0, y0, x1, y1, pull, size) {
  const dx = x1 - x0, dy = y1 - y0, d = Math.hypot(dx, dy);
  if (d < pull + size * 2) return;
  const ux = dx / d, uy = dy / d, tx = x1 - ux * pull, ty = y1 - uy * pull;
  path.moveTo(tx, ty);
  path.lineTo(tx - ux * size - uy * size * 0.45, ty - uy * size + ux * size * 0.45);
  path.lineTo(tx - ux * size + uy * size * 0.45, ty - uy * size - ux * size * 0.45);
  path.closePath();
}

// Edges of one depth slice: thin and faint when far, crisper when near. Batched into a few paths.
function drawEdgeLayer(list, layer, arrows) {
  if (!list.length) return;
  const intro = calm ? 1 : clamp((animNow - born - 200) / 700, 0, 1);
  const t = mode3d ? (layer + 0.5) / LAYERS : 0.35, quiet = (selected >= 0 ? 0.35 : 1) * (edges.length > 1000 ? 0.7 : 1); // dense graphs: fainter lines
  const lines = new Path2D(), heads = new Path2D(), hotLines = new Path2D(), hotHeads = new Path2D();
  let hot = false;
  for (const e of list) {
    const a = nodes[e.a], b = nodes[e.b], isHot = selected === e.a || selected === e.b;
    const path = isHot ? hotLines : lines, head = isHot ? hotHeads : heads;
    if (isHot) hot = true;
    path.moveTo(a.px, a.py); path.lineTo(b.px, b.py);
    if (!arrows && !isHot) continue;
    const size = 3.5 + 2.5 * (1 - t);
    if (e.dir & 1) arrow(head, a.px, a.py, b.px, b.py, b.sr + 1.5, size);
    if (e.dir & 2) arrow(head, b.px, b.py, a.px, a.py, a.sr + 1.5, size);
  }
  ctx.strokeStyle = ctx.fillStyle = css.edge;
  ctx.lineWidth = 0.5 + 0.9 * (1 - t);
  ctx.globalAlpha = (0.1 + 0.32 * (1 - t)) * quiet * intro;
  ctx.stroke(lines);
  if (arrows) {
    ctx.globalAlpha = Math.min(1, ctx.globalAlpha * 1.15);
    ctx.fill(heads);
  }
  if (!hot) return;
  ctx.strokeStyle = ctx.fillStyle = css.accent;
  ctx.lineWidth = 1.2 + 0.6 * (1 - t); ctx.globalAlpha = (0.65 + 0.35 * (1 - t)) * intro;
  ctx.stroke(hotLines); ctx.fill(hotHeads);
}

// Halos for one depth slice, all in one additive pass (dark theme), or soft shadows for the glowing few (light theme).
function drawHalos(list, faint) {
  if (css.dark) {
    ctx.globalCompositeOperation = 'lighter';
    for (const n of list) {
      const strong = glows(n), open = (1 - n.f) * n.g;
      if ((!strong && !faint) || open < 0.03) continue;
      const gs = n.sr * (strong ? 3.6 : 3.0); // 1.8x or 1.5x the ball's radius
      ctx.globalAlpha = (strong ? (n.i === selected ? 1 : 0.75) : 0.8) * open;
      ctx.drawImage(halo(n.ci), n.px - gs / 2, n.py - gs / 2, gs, gs);
    }
    ctx.globalCompositeOperation = 'source-over';
    return;
  }
  for (const n of list) {
    if (!glows(n)) continue;
    const r = n.sr;
    ctx.globalAlpha = 0.3 * n.g;
    ctx.drawImage(shadow(), n.px - r * 1.7, n.py - r * 1.1, r * 3.4, r * 3.4);
  }
}

function drawNode(n, contact, cheapVeil) {
  n.order = drawOrder++;
  const r = n.sr, f = n.f;
  if (contact) { // flat view: a small contact shadow just below the ball
    ctx.globalAlpha = n.g * (css.dark ? 0.45 : 0.18);
    ctx.drawImage(shadow(), n.px - r * 1.2, n.py - r * 0.75, r * 2.4, r * 2.4);
  }
  ctx.globalAlpha = cheapVeil ? n.g * (1 - 0.85 * f) : n.g;
  if (r * dpr > 120) { // huge on screen: paint the gradients directly, no sprite can be sharp enough
    ctx.save(); ctx.translate(n.px, n.py); paintBall(ctx, 0, r, colorOf(n.ci)); ctx.restore();
  } else {
    const need = r * dpr / 0.46, bucket = need > 128 ? 3 : need > 64 ? 2 : need > 32 ? 1 : 0, size = r / 0.46; // the sprite's ball fills 92% of its width
    sprite(n.ci, bucket);
    ctx.drawImage(spr.c, spr.x, spr.y, spr.s, spr.s, n.px - size / 2, n.py - size / 2, size, size);
  }
  if (f > 0.02 && !cheapVeil) {
    ctx.globalAlpha = f * n.g; ctx.fillStyle = css.bg;
    ctx.beginPath(); ctx.arc(n.px, n.py, r + 0.5, 0, 6.2832); ctx.fill();
  }
  if (n.i === selected) {
    ctx.globalAlpha = 1;
    ctx.lineWidth = 2; ctx.strokeStyle = css.bg; ctx.beginPath(); ctx.arc(n.px, n.py, r + 1.5, 0, 6.2832); ctx.stroke();
    ctx.lineWidth = 1.5; ctx.strokeStyle = css.fg; ctx.beginPath(); ctx.arc(n.px, n.py, r + 3.25, 0, 6.2832); ctx.stroke();
    if (ping && ping.i === n.i) { // one ring that eases outward and fades
      const p = clamp((animNow - ping.t0) / 450, 0, 1), e = 1 - Math.pow(1 - p, 3);
      if (p < 1) {
        ctx.globalAlpha = 0.5 * (1 - p); ctx.lineWidth = 1.2; ctx.strokeStyle = css.accent;
        ctx.beginPath(); ctx.arc(n.px, n.py, r + 4 + e * Math.max(8, r * 0.9), 0, 6.2832); ctx.stroke();
      }
    }
  }
}

// Labels go on top, most important first; one that would overlap a placed label is skipped.
function drawLabels(list, found, hubLabels) {
  const want = [], hubs = [];
  for (const n of list) {
    let p = 0;
    if (n.i === selected) p = 4;
    else if (filters.text && found <= 25 && n.m) p = 3;
    else if (selected >= 0 && near.has(n.i) && near.size <= 30 && n.m) p = 2;
    else if (hubLabels && selected < 0 && !filters.text && n.deg >= 0.3 * maxDeg && n.sr >= 5) p = 1;
    if (p > 1) want.push({ n, p });
    else if (p) hubs.push({ n, p });
  }
  // At most 40 hub labels, biggest first: a graph where every file has the same degree would
  // otherwise try to label every ball.
  hubs.sort((a, b) => b.n.deg - a.n.deg);
  want.push(...hubs.slice(0, 40));
  want.sort((a, b) => b.p - a.p || b.n.deg - a.n.deg);
  ctx.font = '500 11.5px system-ui, -apple-system, "Segoe UI", sans-serif';
  ctx.lineJoin = 'round'; ctx.textBaseline = 'middle';
  const placed = [];
  for (const { n, p } of want) {
    const label = baseName(n.path), x = n.px + n.sr + 5, y = n.py, w = ctx.measureText(label).width;
    const box = [x - 2, y - 8, x + w + 2, y + 8];
    if (p < 4 && placed.some(q => box[0] < q[2] && box[2] > q[0] && box[1] < q[3] && box[3] > q[1])) continue;
    placed.push(box);
    ctx.globalAlpha = (p >= 3 ? 1 : 1 - 0.55 * n.t) * n.g;
    ctx.lineWidth = 3; ctx.strokeStyle = css.bg; ctx.strokeText(label, x, y);
    ctx.fillStyle = css.fg; ctx.fillText(label, x, y);
  }
  ctx.globalAlpha = 1;
}

const layerNodes = Array.from({ length: LAYERS }, () => []);
const layerEdges = Array.from({ length: LAYERS }, () => []);
const inView = [], sortBuf = [], focusBuf = [], hotEdges = [];
const SORT_BINS = 256, binStart = new Int32Array(SORT_BINS);
let emptyKey = '', statusKey = '';
const statusEl = document.getElementById('status');
let drawOrder = 0; // order in which balls were painted, so picking can choose the one on top

function chip(name, num, label) {
  const el = document.createElement('span');
  el.className = 'chip';
  el.append(icon(name), text('b', String(num)), document.createTextNode(label));
  return el;
}

