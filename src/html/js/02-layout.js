// Start on a sphere so the layout is deterministic.
nodes.forEach((n, i) => {
  const N = nodes.length, y = 1 - 2 * (i + 0.5) / N, r = Math.sqrt(1 - y * y), t = i * 2.39996;
  const r0 = 40 * Math.cbrt(N) + 20;
  n.x = r0 * r * Math.cos(t); n.y = r0 * y; n.z = r0 * r * Math.sin(t);
});

// Repulsion. Small graphs compare every pair. Bigger ones use a Barnes-Hut octree (a tree of boxes: a far
// group of files is treated as one heavy point), so the cost grows like n log n instead of n squared.
const REPEL = 900, CUTOFF2 = 90000, EXACT_MAX = 300;
function repelExact(n) {
  for (let i = 0; i < n; i++) {
    const a = nodes[i];
    for (let j = i + 1; j < n; j++) {
      const b = nodes[j];
      const dx = a.x - b.x, dy = a.y - b.y, dz = a.z - b.z, d2 = dx * dx + dy * dy + dz * dz + 0.01;
      if (d2 > CUTOFF2) continue;
      const d = Math.sqrt(d2), f = REPEL / d2 / d;
      a.vx += dx * f; a.vy += dy * f; a.vz += dz * f;
      b.vx -= dx * f; b.vy -= dy * f; b.vz -= dz * f;
    }
  }
}
let tree = null;
function makeTree(n) {
  const cap = 8 * n + 64;
  return {
    cap, cells: 0, n,
    cx: new Float64Array(cap), cy: new Float64Array(cap), cz: new Float64Array(cap), h: new Float64Array(cap),
    m: new Int32Array(cap), b: new Int32Array(cap), nc: new Uint8Array(cap), ch: new Int32Array(cap * 8),
    sx: new Float64Array(cap), sy: new Float64Array(cap), sz: new Float64Array(cap),
    x: new Float64Array(n), y: new Float64Array(n), z: new Float64Array(n), stack: new Int32Array(512),
  };
}
function newCell(T, cx, cy, cz, h) {
  const c = T.cells++;
  T.cx[c] = cx; T.cy[c] = cy; T.cz[c] = cz; T.h[c] = h;
  T.m[c] = 0; T.b[c] = -1; T.nc[c] = 0; T.sx[c] = T.sy[c] = T.sz[c] = 0;
  T.ch.fill(-1, c * 8, c * 8 + 8);
  return c;
}
function childOf(T, c, x, y, z) { // the sub-box of c holding the point, created when missing; -1 if the tree is full
  const o = (x > T.cx[c] ? 1 : 0) | (y > T.cy[c] ? 2 : 0) | (z > T.cz[c] ? 4 : 0), slot = c * 8 + o;
  let k = T.ch[slot];
  if (k < 0) {
    if (T.cells >= T.cap) return -1;
    const q = T.h[c] / 2;
    k = newCell(T, T.cx[c] + (o & 1 ? q : -q), T.cy[c] + (o & 2 ? q : -q), T.cz[c] + (o & 4 ? q : -q), q);
    T.ch[slot] = k; T.nc[c]++;
  }
  return k;
}
function insertBody(T, i) {
  const x = T.x[i], y = T.y[i], z = T.z[i];
  let c = 0;
  for (let depth = 0; ; depth++) {
    const first = T.m[c] === 0;
    T.m[c]++; T.sx[c] += x; T.sy[c] += y; T.sz[c] += z;
    if (first) { T.b[c] = i; return; }
    if (T.b[c] >= 0) { // a box holding one file: push that one down a level first
      if (depth >= 30) return; // (almost) the same spot: keep only the sums
      const j = T.b[c], k = childOf(T, c, T.x[j], T.y[j], T.z[j]);
      T.b[c] = -1;
      if (k < 0) return;
      T.m[k] = 1; T.b[k] = j; T.sx[k] = T.x[j]; T.sy[k] = T.y[j]; T.sz[k] = T.z[j];
    }
    c = childOf(T, c, x, y, z);
    if (c < 0) return;
  }
}
function repelTree(n) {
  if (!tree || tree.n !== n) tree = makeTree(n);
  const T = tree;
  let x0 = Infinity, y0 = Infinity, z0 = Infinity, x1 = -Infinity, y1 = -Infinity, z1 = -Infinity;
  for (let i = 0; i < n; i++) {
    const p = nodes[i];
    T.x[i] = p.x; T.y[i] = p.y; T.z[i] = p.z;
    if (p.x < x0) x0 = p.x; if (p.x > x1) x1 = p.x;
    if (p.y < y0) y0 = p.y; if (p.y > y1) y1 = p.y;
    if (p.z < z0) z0 = p.z; if (p.z > z1) z1 = p.z;
  }
  const half = Math.max(x1 - x0, y1 - y0, z1 - z0) / 2 + 1;
  T.cells = 0;
  newCell(T, (x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2, half);
  for (let i = 0; i < n; i++) insertBody(T, i);
  const theta2 = n > 2000 ? 1.8 : 1.0, st = T.stack; // looser grouping for very big graphs
  for (let i = 0; i < n; i++) {
    const xi = T.x[i], yi = T.y[i], zi = T.z[i];
    let fx = 0, fy = 0, fz = 0, sp = 0;
    st[sp++] = 0;
    while (sp > 0) {
      const c = st[--sp], m = T.m[c];
      if (m === 0 || T.b[c] === i) continue;
      const dx = xi - T.sx[c] / m, dy = yi - T.sy[c] / m, dz = zi - T.sz[c] / m, d2 = dx * dx + dy * dy + dz * dz + 0.01;
      const reach = 300 + 3.5 * T.h[c]; // nothing inside this box can be closer than this to be in range
      if (d2 > reach * reach) continue;
      if (T.b[c] >= 0 || T.nc[c] === 0 || 4 * T.h[c] * T.h[c] < theta2 * d2) { // one file, or a group far enough away
        if (d2 > CUTOFF2) continue;
        const f = REPEL * m / (d2 * Math.sqrt(d2));
        fx += dx * f; fy += dy * f; fz += dz * f;
      } else {
        for (let o = 0; o < 8; o++) { const k = T.ch[c * 8 + o]; if (k >= 0) st[sp++] = k; }
      }
    }
    const p = nodes[i];
    p.vx += fx; p.vy += fy; p.vz += fz;
  }
}

let layoutVer = 0;
function step() {
  const n = nodes.length, spring = 0.02, rest = 40, gz = mode3d ? 0.01 : 0.3;
  if (n <= EXACT_MAX) repelExact(n); else repelTree(n);
  for (const e of edges) {
    const a = nodes[e.a], b = nodes[e.b];
    const dx = b.x - a.x, dy = b.y - a.y, dz = b.z - a.z, d = Math.sqrt(dx * dx + dy * dy + dz * dz) + 0.01;
    const f = spring * (d - rest) / d;
    a.vx += dx * f; a.vy += dy * f; a.vz += dz * f;
    b.vx -= dx * f; b.vy -= dy * f; b.vz -= dz * f;
  }
  for (const p of nodes) {
    p.vx -= p.x * 0.01; p.vy -= p.y * 0.01; p.vz -= p.z * gz; // pull to the centre (and flatten in 2D)
    p.x += p.vx * alpha; p.y += p.vy * alpha; p.z += p.vz * alpha;
    p.vx *= 0.6; p.vy *= 0.6; p.vz *= 0.6;
  }
  alpha *= 0.985;
  layoutVer++;
}

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const radius = n => 4 + Math.sqrt(n.deg) * 1.5;
const darkTheme = () => theme === 'dark' || (theme === 'system' && !!matchMedia('(prefers-color-scheme: dark)').matches);
const plural = (n, word, many) => `${n} ${n === 1 ? word : many || word + 's'}`;
const baseName = p => p.split('/').pop();
const ambient = () => css.dark && !calm && !lowQuality && !document.hidden; // drifting stars keep a slow timer alive

