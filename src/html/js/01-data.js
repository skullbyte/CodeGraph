const DATA = /*DATA*/;
const nodes = DATA.nodes.map((n, i) => ({ ...n, i, hay: (n.path + ' ' + n.symbols.map(s => s.name).join(' ')).toLowerCase(), x: 0, y: 0, z: 0, vx: 0, vy: 0, vz: 0, deg: 0, px: 0, py: 0, pz: 0, ps: 1, t: 0, sr: 0, g: 1, ci: -1, vis: true, m: true, f: 0, k: 0, order: -1 }));
const edges = DATA.edges.map(([a, b, w, dir]) => ({ a, b, w, dir }));
// links[i]: neighbours of node i; out = i points at j (imports/calls it), inn = j points at i.
const links = nodes.map(() => []);
for (const e of edges) {
  links[e.a].push({ j: e.b, out: !!(e.dir & 1), inn: !!(e.dir & 2) });
  links[e.b].push({ j: e.a, out: !!(e.dir & 2), inn: !!(e.dir & 1) });
}
nodes.forEach(n => { n.deg = links[n.i].length; });
const maxDeg = nodes.reduce((m, n) => Math.max(m, n.deg), 1);
// The few best-connected files get a stronger halo in the dark theme.
const hubSet = new Set(nodes.filter(n => n.deg >= 0.35 * maxDeg && n.deg >= 3).sort((a, b) => b.deg - a.deg).slice(0, 14).map(n => n.i));

// Categorical colours: 16 well-separated hues per theme (the dark set is a little lighter). Communities get
// them in size order. Past 16 the hues repeat shifted along the list and lighter or darker, so neighbours differ.
const PALETTE = {
  light: ['#3f74c0', '#d4892c', '#23977e', '#c94f4a', '#7a62bf', '#7c9c3a', '#cc6a98', '#2a9bb8', '#4a9e4f', '#dc6f3c', '#a553b3', '#b89a1f', '#5360cf', '#94694c', '#6b86a8', '#9c3d5d'],
  dark: ['#5584cf', '#c4822c', '#2ea78b', '#d65f59', '#8b76d0', '#7f9d3c', '#c96f99', '#35aecb', '#55b05a', '#e07a48', '#b866c6', '#cdb02f', '#6573e0', '#a77c5e', '#7f9bbd', '#b44f72'],
};
const comms = new Map();
for (const n of nodes) {
  const c = comms.get(n.community) || { id: n.community, count: 0, top: n, ci: -1 };
  c.count++;
  if (n.deg > c.top.deg) c.top = n;
  comms.set(n.community, c);
}
const commList = [...comms.values()].sort((a, b) => b.count - a.count || a.id - b.id);
commList.forEach((c, rank) => { c.ci = rank; }); // every community gets its own colour
nodes.forEach(n => { n.ci = comms.get(n.community).ci; });
const hidden = new Set(); // communities switched off in the legend

// Icons: small stroke shapes drawn as SVG (no images). Paths are in a 24x24 box.
const ICONS = {
  search: ['M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z', 'M21 21l-4.3-4.3'],
  folder: ['M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z'],
  tag: ['M3 12V4h8l9 9-8 8z', 'M7.5 8.5h.01'],
  layers: ['M12 3 3 8l9 5 9-5z', 'M3 13l9 5 9-5'],
  square: ['M6 4h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z'],
  sun: ['M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z', 'M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4'],
  moon: ['M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z'],
  monitor: ['M5 4h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z', 'M8 21h8M12 17v4'],
  reset: ['M3 12a9 9 0 1 0 2.6-6.4L3 8', 'M3 3v5h5'],
  chev: ['M6 9l6 6 6-6'],
  chevR: ['M9 6l6 6-6 6'],
  check: ['M5 12.5l4.5 4.5L19 7.5'],
  close: ['M6 6l12 12M18 6 6 18'],
  file: ['M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z', 'M14 3v5h5'],
  code: ['M8 8l-5 4 5 4M16 8l5 4-5 4'],
  arrowR: ['M5 12h14M13 6l6 6-6 6'],
  arrowL: ['M19 12H5M11 6l-6 6 6 6'],
  both: ['M4 12h16M8 8l-4 4 4 4M16 8l4 4-4 4'],
  nodes: ['M6 4a2.5 2.5 0 1 0 .01 0z', 'M18 9a2.5 2.5 0 1 0 .01 0z', 'M9 18a2.5 2.5 0 1 0 .01 0z', 'M8 6.5l8 2M7.3 7l1.2 8.5M16.5 11l-5 5.5'],
  link: ['M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1', 'M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1'],
};
const SVG_NS = 'http:/' + '/www.w3.org/2000/svg'; // split so the page contains no URL-looking text
function icon(name) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('class', 'ic'); svg.setAttribute('aria-hidden', 'true');
  for (const d of ICONS[name] || []) {
    const p = document.createElementNS(SVG_NS, 'path');
    p.setAttribute('d', d); svg.append(p);
  }
  return svg;
}
document.querySelectorAll('[data-icon]').forEach(el => el.prepend(icon(el.dataset.icon)));

const canvas = document.getElementById('canvas');
const ctx = canvas.getContext('2d');
const bgc = document.getElementById('bg');     // the space backdrop lives on its own canvas under the graph
const bctx = bgc.getContext('2d');
const view = { x: 0, y: 0, k: 1 };
const rot = { x: 0.55, y: 0.6 }; // positive x tilt looks down onto the floor
const filters = { text: '', folder: '', kind: '' };
let mode3d = true;
let selected = -1;
let near = new Set();  // neighbours of the selected node
let alpha = 1;
let R = 100;           // radius of the layout, for fitting and depth shading
let D = 300;           // camera distance; smaller = stronger perspective
let fitK = 1;          // zoom chosen by fit(), the zero point for the star parallax
let userMoved = false; // once set, the layout stops re-fitting the view
let fitDirty = true;    // the layout radius must be measured again
let css = {};
let dpr = 1;
let dirty = true;       // the graph canvas must be repainted
const calm = !!matchMedia('(prefers-reduced-motion: reduce)').matches; // no spin, intro, glide, ripple or drifting stars
let drawCount = 0, slowStreak = 0, glideId = 0;
let lowQuality = false; // switched on automatically when frames get slow
let frameMs = 0;        // smoothed draw time
let animNow = 0;        // time of the current frame, for the intro animation
const born = performance.now();
let shadowSprite = null, floorGlow = null, backdrop = null, noiseTile = null;
const LAYERS = 4;       // depth slices: far edges, far balls, ..., near edges, near balls
const SPIN_DELAY = 3000; // spin pauses on any interaction and resumes this long after the last one

// Visible files (not hidden by a filter), cached: recomputed only when a filter changes.
let shown = nodes.slice(), shownEdges = edges.slice(), foundCount = nodes.length;

