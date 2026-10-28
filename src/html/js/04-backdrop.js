// ---- Space backdrop (own canvas under the graph) -------------------------------------------------
// The nebula is painted once per size and theme at half the screen's pixel density (it is all soft gradients),
// with a whisper of noise on top so the dark gradient does not show bands. Three star layers are small tiles
// painted once; each repaint they are just shifted and tiled. The graph canvas is not touched by any of this.
const BM = 24; // margin of the nebula image, so it can sway a little (parallax) without showing edges
function grain() { // 64x64 tile of black and white specks that nearly cancel out: dithers the gradient steps
  if (noiseTile) return noiseTile;
  noiseTile = makeCanvas(64);
  const g = noiseTile.getContext('2d'), img = g.createImageData(64, 64);
  let seed = 12345;
  for (let i = 0; i < 64 * 64; i++) {
    seed = (Math.imul(seed, 1664525) + 1013904223) | 0;
    const v = (seed >>> 24) & 1 ? 255 : 0;
    img.data[i * 4] = img.data[i * 4 + 1] = img.data[i * 4 + 2] = v; img.data[i * 4 + 3] = 5;
  }
  g.putImageData(img, 0, 0);
  return noiseTile;
}
function buildBackdrop(w, h) {
  const scale = 0.5 * bgc.width / Math.max(w, 1), W = (w + 2 * BM) * scale, H = (h + 2 * BM) * scale;
  const el = makeCanvas(W, H), g = el.getContext('2d');
  const sky = g.createLinearGradient(0, 0, 0, el.height);
  sky.addColorStop(0, css.bgTop); sky.addColorStop(1, css.bgBottom);
  g.fillStyle = sky; g.fillRect(0, 0, el.width, el.height);
  const big = Math.max(el.width, el.height);
  const glow = (x, y, r, color) => {
    const gr = g.createRadialGradient(x * el.width, y * el.height, 0, x * el.width, y * el.height, r * big);
    gr.addColorStop(0, color); gr.addColorStop(1, 'rgba(0,0,0,0)');
    g.fillStyle = gr; g.fillRect(0, 0, el.width, el.height);
  };
  if (css.dark) {
    glow(0.74, 0.2, 0.55, 'rgba(70,84,214,.2)');
    glow(0.14, 0.82, 0.5, 'rgba(24,126,158,.13)');
    glow(0.5, 0.45, 0.65, 'rgba(104,58,166,.08)');
    glow(0.92, 0.88, 0.4, 'rgba(40,70,190,.1)');
    g.save(); // a faint diagonal band of light, like a galactic arm
    g.translate(el.width / 2, el.height / 2); g.rotate(-0.42);
    const band = g.createLinearGradient(0, -big * 0.2, 0, big * 0.2);
    band.addColorStop(0, 'rgba(150,170,255,0)'); band.addColorStop(0.5, 'rgba(150,170,255,.05)'); band.addColorStop(1, 'rgba(150,170,255,0)');
    g.fillStyle = band; g.fillRect(-big, -big * 0.2, 2 * big, big * 0.4);
    g.restore();
    const vig = g.createRadialGradient(el.width / 2, el.height * 0.47, Math.min(el.width, el.height) * 0.3, el.width / 2, el.height * 0.47, Math.hypot(el.width, el.height) * 0.62);
    vig.addColorStop(0, 'rgba(0,0,0,0)'); vig.addColorStop(1, 'rgba(0,0,0,.6)');
    g.fillStyle = vig; g.fillRect(0, 0, el.width, el.height);
  } else {
    glow(0.5, 0.35, 0.7, 'rgba(255,255,255,.55)');
    const vig = g.createRadialGradient(el.width / 2, el.height / 2, Math.min(el.width, el.height) * 0.4, el.width / 2, el.height / 2, Math.hypot(el.width, el.height) * 0.62);
    vig.addColorStop(0, 'rgba(60,50,30,0)'); vig.addColorStop(1, 'rgba(60,50,30,.07)');
    g.fillStyle = vig; g.fillRect(0, 0, el.width, el.height);
  }
  if (css.dark) { g.fillStyle = g.createPattern(grain(), 'repeat'); g.fillRect(0, 0, el.width, el.height); } // the pale theme has no visible banding
  return el;
}

// Star layers, far to near: tile size (css px), count, star radius range, drift speed (px/s), parallax strength.
const STAR_LAYERS = [
  { tile: 520, n: 150, r: [0.35, 0.75], a: 0.6, speed: 1.5, par: 0.12 },
  { tile: 680, n: 70, r: [0.6, 1.05], a: 0.8, speed: 4, par: 0.3 },
  { tile: 880, n: 26, r: [0.9, 1.6], a: 1, speed: 9, par: 0.7, soft: true },
];
let stars = null, starScale = 1;
function buildStars() {
  starScale = Math.min(window.devicePixelRatio || 1, 1.5);
  let seed = 20240607;
  const rnd = () => { seed = (seed + 0x6d2b79f5) | 0; let t = Math.imul(seed ^ (seed >>> 15), 1 | seed); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
  const tints = ['255,255,255', '205,220,255', '255,236,215', '190,205,255'];
  stars = STAR_LAYERS.map(L => {
    const size = L.tile * starScale, el = makeCanvas(size), g = el.getContext('2d');
    for (let i = 0; i < L.n; i++) {
      const x = rnd() * size, y = rnd() * size, r = (L.r[0] + rnd() * (L.r[1] - L.r[0])) * starScale;
      const tint = tints[Math.floor(rnd() * tints.length)], a = L.a * (0.45 + 0.55 * rnd());
      if (L.soft) { // the near stars get a faint bloom
        const gr = g.createRadialGradient(x, y, 0, x, y, r * 5);
        gr.addColorStop(0, `rgba(${tint},${a * 0.35})`); gr.addColorStop(1, `rgba(${tint},0)`);
        g.fillStyle = gr; g.fillRect(x - r * 5, y - r * 5, r * 10, r * 10);
      }
      g.fillStyle = `rgba(${tint},${a})`; g.beginPath(); g.arc(x, y, r, 0, 6.2832); g.fill();
    }
    return { L, pattern: bctx.createPattern(el, 'repeat') };
  });
}

function drawStars(w, h, now) {
  if (!stars || starScale !== Math.min(window.devicePixelRatio || 1, 1.5)) buildStars();
  const t = ambient() ? now / 1000 : 0, zoom = clamp(Math.log(view.k / fitK), -1.5, 1.5);
  for (const { L, pattern } of stars) {
    const s = (1 + zoom * 0.1 * L.par) / starScale; // zooming in moves near stars apart a little more than far ones
    const ox = w / 2 - t * L.speed - rot.y * L.par * 70 - view.x * L.par * 0.1;
    const oy = h / 2 + rot.x * L.par * 50 + view.y * L.par * 0.1;
    pattern.setTransform(new DOMMatrix([s, 0, 0, s, ox, oy]));
    bctx.fillStyle = pattern; bctx.fillRect(0, 0, w, h);
  }
}

// Repaints the backdrop only when something it shows has moved by a visible amount (a quarter of a pixel):
// a still graph in the light theme, calm mode or a hidden tab costs nothing here.
let resizeTimer = 0, bgVer = 0;
const bgSig = new Float64Array(9), bgNew = new Float64Array(9);
function paintBg(now) {
  if (!bgc.width || !bgc.height || !css.bg) return;
  const w = canvas.clientWidth, h = canvas.clientHeight, dark = css.dark;
  bgNew[0] = bgVer; bgNew[1] = w; bgNew[2] = h; bgNew[3] = bgc.width;
  bgNew[4] = dark ? (ambient() ? Math.floor(now / 50) : 0) : 0; // drift ticks at 20 per second
  bgNew[5] = dark ? Math.round(rot.y * 200) : 0; bgNew[6] = dark ? Math.round(rot.x * 200) : 0;
  bgNew[7] = dark ? Math.round(view.x * 0.3) * 1000 + Math.round(view.y * 0.3) : 0;
  bgNew[8] = dark ? Math.round(Math.log(view.k / fitK) * 100) : 0;
  let same = true;
  for (let i = 0; i < 9; i++) if (bgNew[i] !== bgSig[i]) { same = false; break; }
  if (same) return;
  bgSig.set(bgNew);
  const key = w + 'x' + h + 'x' + bgc.width + (dark ? 'd' : 'l');
  if (!backdrop || (backdrop.key !== key && !resizeTimer)) { backdrop = buildBackdrop(w, h); backdrop.key = key; } // while the window is being dragged the old image is just stretched
  const dx = dark ? Math.sin(rot.y * 0.5) * 9 - view.x * 0.02 : 0, dy = dark ? (rot.x - 0.55) * 12 - view.y * 0.02 : 0;
  bctx.drawImage(backdrop, -BM + clamp(dx, -BM, BM), -BM + clamp(dy, -BM, BM), w + 2 * BM, h + 2 * BM);
  if (dark) drawStars(w, h, now);
}

const visible = n =>
  !hidden.has(n.community) &&
  (!filters.folder || n.folder === filters.folder) &&
  (!filters.kind || n.symbols.some(s => s.kind === filters.kind));
const matches = n => n.m;

