// Colours. Parse #rrggbb, mix two of them (t = 0 gives a, t = 1 gives b).
const rgbOf = h => [1, 3, 5].map(k => parseInt(h.slice(k, k + 2), 16));
function mix(a, b, t, op = 1) {
  const x = rgbOf(a), y = rgbOf(b);
  return `rgba(${x.map((v, k) => Math.round(v + (y[k] - v) * t)).join(',')},${op})`;
}
function hexMix(a, b, t) {
  const x = rgbOf(a), y = rgbOf(b);
  return '#' + x.map((v, k) => Math.round(v + (y[k] - v) * t).toString(16).padStart(2, '0')).join('');
}
const colorCache = new Map();
const COLOR_SLOTS = 64; // colours (and their sprites) repeat after this many communities, so memory stays small
function colorOf(ci) {
  ci %= COLOR_SLOTS;
  const th = css.dark ? 'dark' : 'light', key = ci * 2 + (css.dark ? 1 : 0);
  let c = colorCache.get(key);
  if (c) return c;
  const pal = PALETTE[th], n = pal.length, round = Math.floor(ci / n);
  c = pal[(((ci % n) + 5 * round) % n + n) % n];
  if (round > 0) { // later rounds: a different hue slot, alternately lighter and darker
    const s = 0.16 + 0.04 * Math.min(round, 3);
    c = round % 2 ? hexMix(c, '#ffffff', s) : hexMix(c, '#000000', s);
  }
  colorCache.set(key, c);
  return c;
}

const makeCanvas = (w, h = w) => { const el = document.createElement('canvas'); el.width = Math.max(1, Math.round(w)); el.height = Math.max(1, Math.round(h)); return el; };

// Satin ball painted around (m, m) with the given radius. Used for the cached sprites, and directly for huge balls.
function paintBall(g, m, r, base) {
  const dark = css.dark;
  const body = g.createRadialGradient(m - r * 0.35, m - r * 0.4, r * 0.05, m, m, r * 1.05);
  body.addColorStop(0, mix(base, '#ffffff', dark ? 0.2 : 0.26));
  body.addColorStop(0.5, base);
  body.addColorStop(1, mix(base, '#000000', dark ? 0.4 : 0.28));
  g.fillStyle = body; g.beginPath(); g.arc(m, m, r, 0, 6.2832); g.fill();
  const sheen = g.createRadialGradient(m - r * 0.32, m - r * 0.38, 0, m - r * 0.32, m - r * 0.38, r * 0.5);
  sheen.addColorStop(0, `rgba(255,255,255,${dark ? 0.22 : 0.3})`); sheen.addColorStop(1, 'rgba(255,255,255,0)');
  g.fillStyle = sheen; g.beginPath(); g.arc(m, m, r, 0, 6.2832); g.fill();
  g.lineWidth = Math.max(0.75, r * 0.034); g.strokeStyle = mix(base, '#000000', 0.45, dark ? 0.5 : 0.35);
  g.beginPath(); g.arc(m, m, r - g.lineWidth / 2, 0, 6.2832); g.stroke();
}

// Pre-rendered balls: one per community and size bucket (32/64/128/256 px), built the first time they are needed,
// so a zoomed-in ball is drawn from a sprite at least as sharp as the screen. The three small buckets share one
// atlas canvas per size (a sheet of sprites): drawing many balls from one sheet avoids switching images.
const SPRITE_PX = [32, 64, 128, 256], ATLAS_COLS = 16;
let spriteTab = [[], [], [], []], atlasTab = [null, null, null], haloTab = [];
const spr = { c: null, x: 0, y: 0, s: 32 }; // result of sprite(): which canvas, which square of it
function sprite(ci, bucket) {
  ci %= COLOR_SLOTS;
  const S = SPRITE_PX[bucket], sheet = bucket < 3;
  spr.s = S;
  if (sheet) {
    let at = atlasTab[bucket];
    if (!at) at = atlasTab[bucket] = makeCanvas(ATLAS_COLS * S, Math.ceil(Math.min(comms.size, COLOR_SLOTS) / ATLAS_COLS) * S);
    spr.c = at; spr.x = (ci % ATLAS_COLS) * S; spr.y = Math.floor(ci / ATLAS_COLS) * S;
    if (!spriteTab[bucket][ci]) {
      const g = at.getContext('2d');
      g.save(); g.translate(spr.x, spr.y); paintBall(g, S / 2, S * 0.46, colorOf(ci)); g.restore();
      spriteTab[bucket][ci] = true;
    }
    return;
  }
  let sp = spriteTab[bucket][ci];
  if (!sp) {
    sp = spriteTab[bucket][ci] = makeCanvas(S);
    paintBall(sp.getContext('2d'), S / 2, S * 0.46, colorOf(ci));
  }
  spr.c = sp; spr.x = 0; spr.y = 0;
}

// A soft halo per colour (dark theme): drawn additively, 1.5x the ball for every ball and 1.8x for hubs and the selection.
function halo(ci) {
  ci %= COLOR_SLOTS;
  let sp = haloTab[ci];
  if (sp) return sp;
  const base = colorOf(ci), S = 128, m = S / 2;
  sp = makeCanvas(S);
  const g = sp.getContext('2d'), grad = g.createRadialGradient(m, m, 0, m, m, m);
  for (const t of [0, 0.15, 0.3, 0.5, 0.7, 0.85, 1]) grad.addColorStop(t, mix(base, '#ffffff', 0.12, Math.pow(1 - t, 1.5)));
  g.fillStyle = grad; g.fillRect(0, 0, S, S);
  haloTab[ci] = sp;
  return sp;
}

// A soft round shadow, squashed when drawn onto the floor.
function shadow() {
  if (shadowSprite) return shadowSprite;
  const S = 128, m = S / 2;
  shadowSprite = makeCanvas(S);
  const g = shadowSprite.getContext('2d'), grad = g.createRadialGradient(m, m, 0, m, m, m);
  grad.addColorStop(0, mix(css.shadow, css.shadow, 0, 1));
  grad.addColorStop(0.45, mix(css.shadow, css.shadow, 0, 0.55));
  grad.addColorStop(1, mix(css.shadow, css.shadow, 0, 0));
  g.fillStyle = grad; g.fillRect(0, 0, S, S);
  return shadowSprite;
}

