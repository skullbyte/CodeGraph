// Cull the files off screen, sort the rest far to near (a counting sort, so O(n)), and deal them into depth slices.
function buildLists(L, w, h) {
  for (let k = 0; k < L; k++) { layerNodes[k].length = 0; layerEdges[k].length = 0; }
  inView.length = 0; sortBuf.length = 0; focusBuf.length = 0; hotEdges.length = 0;
  binStart.fill(0);
  for (const n of shown) {
    const r = n.sr + 6;
    if (n.px < -r || n.px > w + r || n.py < -r || n.py > h + r) continue;
    n.k = SORT_BINS - 1 - ((n.t * (SORT_BINS - 1)) | 0); // far files get the small numbers: they are painted first
    binStart[n.k]++;
    inView.push(n);
  }
  let acc = 0;
  for (let b = 0; b < SORT_BINS; b++) { const c = binStart[b]; binStart[b] = acc; acc += c; }
  sortBuf.length = inView.length;
  for (const n of inView) sortBuf[binStart[n.k]++] = n;
  const slice = t => Math.min(L - 1, Math.floor(t * L));
  // With a selection, the selected file, its neighbours and their links are drawn last, so the
  // veiled rest of the graph never covers them.
  for (const n of sortBuf) {
    n.f = veil(n);
    if (selected >= 0 && (n.i === selected || near.has(n.i)) && n.m) focusBuf.push(n);
    else layerNodes[L - 1 - slice(n.t)].push(n);
  }
  for (const e of shownEdges) {
    const a = nodes[e.a], b = nodes[e.b];
    if ((a.px < 0 && b.px < 0) || (a.px > w && b.px > w) || (a.py < 0 && b.py < 0) || (a.py > h && b.py > h)) continue;
    if (selected === e.a || selected === e.b) hotEdges.push(e);
    else layerEdges[L - 1 - slice((a.t + b.t) / 2)].push(e);
  }
}

function draw() {
  dirty = false;
  if (!canvas.width || !canvas.height) return; // hidden or collapsed: nothing to paint, and drawImage would throw
  if (!css.bg) readColors(); // select(-1) can run before the theme is applied
  const t0 = performance.now(), w = canvas.clientWidth, h = canvas.clientHeight;
  animNow = t0;
  ctx.clearRect(0, 0, w, h);
  ctx.globalAlpha = 1;
  drawOrder = 0;
  for (const n of nodes) n.order = -1;
  projectAll(!calm && t0 - born < 1300);
  // Level of detail: big graphs drop the finest touches (faint halos, shadows, arrows, most labels).
  const lod = shown.length > 1500, faint = !lowQuality && shown.length <= 1200;
  const L = mode3d ? LAYERS : 1;
  buildLists(L, w, h);
  if (mode3d && !lowQuality) drawFloor(sortBuf, !lod);
  const arrows = !lowQuality && shownEdges.length <= 4000 && view.k >= fitK * 0.5;
  const contact = !mode3d && !lowQuality && !lod;
  for (let k = 0; k < L; k++) {
    drawEdgeLayer(layerEdges[k], L - 1 - k, arrows);
    const list = layerNodes[k];
    if (!lowQuality) drawHalos(list, faint);
    ctx.globalAlpha = 1;
    for (const n of list) drawNode(n, contact, lod);
  }
  drawEdgeLayer(hotEdges, 0, arrows);
  if (!lowQuality) drawHalos(focusBuf, faint);
  ctx.globalAlpha = 1;
  for (const n of focusBuf) drawNode(n, contact, lod);
  drawLabels(inView, foundCount, !lod || view.k >= fitK * 0.8);
  ctx.globalAlpha = 1;
  if (++drawCount > 10) { // the first draws build sprites and warm up the JIT
    frameMs = frameMs * 0.9 + (performance.now() - t0) * 0.1;
    slowStreak = frameMs > 22 ? slowStreak + 1 : 0;
    if (slowStreak >= 30) lowQuality = true; // slow machine: drop the floor, glow, shadows, arrows and drifting stars
  }
  const key = `${foundCount}/${nodes.length}/${edges.length}/${comms.size}`;
  if (statusKey !== key) { // live region: only touch it on change
    statusKey = key;
    statusEl.replaceChildren(
      chip('file', foundCount === nodes.length ? nodes.length : `${foundCount} of ${nodes.length}`, nodes.length === 1 ? ' file' : ' files'),
      chip('link', edges.length, edges.length === 1 ? ' link' : ' links'),
      chip('nodes', comms.size, comms.size === 1 ? ' community' : ' communities'),
    );
  }
  showEmpty(shown.length, foundCount);
}

// Friendly message over the canvas when there is nothing to draw.
function showEmpty(shownCount, found) {
  const box = document.getElementById('empty');
  const key = !nodes.length ? 'none' : !shownCount ? 'filtered' : !found ? 'search:' + filters.text : '';
  if (key === emptyKey) return;
  emptyKey = key;
  box.replaceChildren();
  box.hidden = !key;
  if (!key) return;
  if (key === 'none') {
    box.append(text('h2', 'No files to show'), text('p', 'Run codegraph index in your project, then codegraph html again.'));
    return;
  }
  const searching = key !== 'filtered';
  box.append(
    text('h2', searching ? 'No matches' : 'Everything is filtered out'),
    text('p', searching ? `No file or function name contains "${filters.text}".` : 'The folder, kind or community filters hide every file.'),
  );
  const btn = text('button', searching ? 'Clear search' : 'Show all files', 'btn');
  btn.type = 'button';
  btn.onclick = () => {
    if (searching) { search.value = ''; filters.text = ''; filtersChanged(); } else { resetFilters(); invalidate(); }
  };
  box.append(btn);
}

function pick(px, py) {
  let best = -1, top = -1; // the ball painted last (on top) wins, also in the flat view where all depths are equal
  for (const n of nodes) {
    if (n.order < 0) continue;
    const dx = n.px - px, dy = n.py - py, r = n.sr + 3;
    if (dx * dx + dy * dy <= r * r && n.order > top) { best = n.i; top = n.order; }
  }
  return best;
}

function text(tag, content, cls) {
  const el = document.createElement(tag);
  el.textContent = content;
  if (cls) el.className = cls;
  return el;
}

function slideIn(panel) { // restart the CSS enter animation
  panel.classList.remove('in'); void panel.offsetWidth; panel.classList.add('in');
}

let ping = null; // one-off ring around a freshly selected node
let rippling = false, gliding = false;
function rippleLoop() { // one chain at most; it only asks for repaints
  if (rippling) return;
  rippling = true;
  const tick = () => {
    invalidate();
    if (ping && performance.now() - ping.t0 < 460) requestAnimationFrame(tick);
    else rippling = false;
  };
  requestAnimationFrame(tick);
}
const cancelGlide = () => { glideId++; gliding = false; };

const KINDS = { Fn: 'function', Struct: 'struct', Enum: 'enum', Trait: 'trait', Impl: 'impl', Const: 'constant', Static: 'static', Type: 'type', Mod: 'module', Macro: 'macro', Interface: 'interface', Class: 'class', Method: 'method', Var: 'variable' };
const kindName = k => KINDS[k] || k.toLowerCase();
const kindPlural = k => { const s = kindName(k); return /(s|x|z|ch|sh)$/.test(s) ? s + 'es' : s + 's'; };
const kindLabel = k => { const s = kindPlural(k); return s[0].toUpperCase() + s.slice(1); }; // "Constants", "Functions", ...

function welcome(panel) {
  panel.append(text('h2', 'Explore the graph'),
    text('p', 'Each ball is a source file. Lines join files that use each other; bigger balls have more links. Colours group files that work closely together.', 'lede'));
  panel.append(text('h3', 'Controls'));
  const tips = document.createElement('dl');
  tips.className = 'tips group';
  for (const [key, what] of [['Click', 'Open a file to see its functions and links'], ['Drag', 'Rotate the 3D view'], ['Shift + drag', 'Move around (or right-drag)'], ['Scroll', 'Zoom in and out (pinch on touch)'], ['/', 'Focus the search box'], ['Enter', 'In search, jump to the first match'], ['Esc', 'Close the selected file']]) {
    const dt = document.createElement('dt');
    dt.append(text('kbd', key));
    tips.append(dt, text('dd', what));
  }
  panel.append(tips);
}

function sec(title, count) {
  const h = text('h3', title);
  h.append(text('span', String(count), 'count'));
  return h;
}

function linkList(i, title, keep, arrowName) {
  const items = links[i].filter(keep);
  const list = document.createElement('ul');
  list.className = 'group';
  for (const l of items) {
    const li = document.createElement('li'), btn = document.createElement('button'), p = nodes[l.j].path, cut = p.lastIndexOf('/');
    btn.type = 'button'; btn.className = 'file'; btn.title = p;
    const path = document.createElement('span');
    path.className = 'path';
    path.append(text('span', baseName(p)));
    if (cut > 0) path.append(text('span', p.slice(0, cut), 'dir'));
    btn.append(icon(l.out && l.inn ? 'both' : arrowName), path);
    btn.onclick = () => {
      if (!visible(nodes[l.j])) resetFilters(); // a hidden file can't be shown or centred
      select(l.j); center(l.j);
    };
    li.append(btn);
    list.append(li);
  }
  if (!items.length) list.append(text('li', 'None', 'none'));
  return [sec(title, items.length), list];
}

let panelFor = -2; // which selection the side panel currently shows (-2: nothing built yet)
function select(i) {
  if (i === panelFor) { invalidate(); return; } // same selection: leave the panel alone (no rebuild, no slide-in)
  panelFor = i;
  selected = i;
  near = new Set(i >= 0 ? links[i].map(l => l.j) : []);
  const panel = document.getElementById('panel');
  panel.replaceChildren();
  panel.scrollTop = 0;
  if (i < 0) { ping = null; welcome(panel); slideIn(panel); invalidate(); return; }
  const n = nodes[i], parts = n.path.split('/');
  const top = document.createElement('div'), crumbs = document.createElement('div');
  top.className = 'top'; crumbs.className = 'crumbs';
  parts.slice(0, -1).forEach(part => crumbs.append(text('span', part), icon('chevR')));
  if (parts.length === 1) crumbs.append(text('span', 'project root'));
  const close = document.createElement('button');
  close.type = 'button'; close.className = 'ibtn'; close.title = 'Close (Esc)'; close.setAttribute('aria-label', 'Close selection');
  close.append(icon('close')); close.onclick = () => select(-1);
  top.append(crumbs, close);
  const title = document.createElement('div'), dot = document.createElement('span');
  title.className = 'title'; dot.className = 'dot'; dot.style.background = colorOf(n.ci);
  title.append(dot, text('h2', baseName(n.path)));
  const uses = links[i].filter(l => l.out).length, usedBy = links[i].filter(l => l.inn).length;
  const chips = document.createElement('div');
  chips.className = 'chips';
  const comm = document.createElement('span'), cdot = document.createElement('span');
  comm.className = 'chip'; cdot.className = 'dot'; cdot.style.background = colorOf(n.ci);
  comm.append(cdot, document.createTextNode('Community '), text('b', String(n.community)));
  chips.append(chip('code', n.symbols.length, n.symbols.length === 1 ? ' symbol' : ' symbols'), chip('arrowR', uses, ' uses'), chip('arrowL', usedBy, ' used by'), comm);
  panel.append(top, title, chips);
  const syms = document.createElement('ul');
  syms.className = 'group';
  for (const s of n.symbols) {
    const li = document.createElement('li');
    li.className = 'sym';
    li.title = `${n.path}:${s.line}`;
    li.append(text('span', kindName(s.kind), 'kind'), text('b', s.name), text('span', `line ${s.line}`, 'ln'));
    syms.append(li);
  }
  if (!n.symbols.length) syms.append(text('li', 'No symbols found in this file', 'none'));
  panel.append(sec('Symbols', n.symbols.length), syms, ...linkList(i, 'Uses', l => l.out, 'arrowR'), ...linkList(i, 'Used by', l => l.inn, 'arrowL'));
  slideIn(panel);
  if (!calm) { ping = { i, t0: performance.now() }; rippleLoop(); }
  invalidate();
}

function center(i) {
  userMoved = true;
  interact(); // spin stays paused while the view glides, and for 3 s after
  projectAll(false);
  const dx = canvas.clientWidth / 2 - nodes[i].px, dy = canvas.clientHeight / 2 - nodes[i].py;
  if (calm) { view.x += dx; view.y += dy; invalidate(); return; }
  const x0 = view.x, y0 = view.y, t0 = performance.now(), id = ++glideId;
  gliding = true;
  const glide = now => { // ease-in-out over 420 ms instead of jumping
    if (id !== glideId) return; // cancelled by a drag, zoom or view switch
    const t = clamp((now - t0) / 420, 0, 1), e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
    view.x = x0 + dx * e; view.y = y0 + dy * e;
    invalidate();
    if (t < 1) requestAnimationFrame(glide);
    else gliding = false;
  };
  requestAnimationFrame(glide);
}

function fit() {
  if (!nodes.length) return;
  // Size the view to the bulk of the graph, so a few far-flung loners don't shrink everything.
  // Only re-measured while the layout still moves, or after a view or size change.
  if (alpha > 0.02 || fitDirty) {
    const dist = nodes.map(n => Math.hypot(n.x, n.y, mode3d ? n.z : 0)).sort((p, q) => p - q);
    R = Math.max(1, dist[Math.ceil((dist.length - 1) * 0.9)] * 1.1);
    fitDirty = false;
  }
  D = 2.4 * R + 60;
  // Fill about 80% of the view: width and height are both used, so wide screens are not left empty.
  const w = canvas.clientWidth, h = canvas.clientHeight;
  view.k = clamp(Math.min(w * FIT_W, h * FIT_H) / (mode3d ? R : R * 1.05), 0.05, 3);
  fitK = view.k;
  view.x = 0; view.y = mode3d ? -R * 0.06 * view.k : 0; // leave a little room for the floor below
}
const FIT_W = 0.5, FIT_H = 0.44; // radius of the graph as a share of the view (about 80% across)

function resetFilters() {
  filters.folder = ''; filters.kind = '';
  dropdowns.forEach(d => d.set(''));
  hidden.clear();
  legendBoxes.forEach(box => { box.checked = true; });
  refreshVis();
}

