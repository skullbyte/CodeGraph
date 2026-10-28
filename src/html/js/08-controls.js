// Community legend with a checkbox per community, like Graphify's. Lives at the bottom of the side panel;
// collapsed by default on phones.
const legendBoxes = [], legendDots = [];
function buildLegend() {
  const legend = document.getElementById('legend');
  if (!comms.size) { legend.style.display = 'none'; return; }
  const head = document.createElement('div'), list = document.createElement('div');
  head.className = 'head'; list.className = 'list';
  const toggle = document.createElement('button');
  toggle.type = 'button'; toggle.className = 'toggle';
  toggle.append(icon('nodes'), text('span', 'Communities'), text('span', String(comms.size), 'count'));
  const setOpen = open => { legend.classList.toggle('closed', !open); toggle.setAttribute('aria-expanded', String(open)); };
  toggle.onclick = () => setOpen(legend.classList.contains('closed'));
  setOpen(window.innerWidth > 760);
  const all = text('button', 'All', 'link'), none = text('button', 'None', 'link');
  all.type = none.type = 'button';
  all.setAttribute('aria-label', 'Show all communities'); none.setAttribute('aria-label', 'None, hide all communities');
  const setAll = on => {
    hidden.clear();
    if (!on) comms.forEach((_, id) => hidden.add(id));
    legendBoxes.forEach(box => { box.checked = on; });
    filtersChanged();
  };
  all.onclick = () => setAll(true); none.onclick = () => setAll(false);
  head.append(toggle, all, none);
  for (const c of commList) {
    const row = document.createElement('label'), box = document.createElement('input'), dot = document.createElement('span');
    box.type = 'checkbox'; box.checked = true; legendBoxes.push(box);
    dot.className = 'dot'; legendDots.push([dot, c.ci]);
    row.title = `Community ${c.id}: ${plural(c.count, 'file')}, biggest is ${c.top.path}`;
    box.onchange = () => {
      if (box.checked) hidden.delete(c.id); else hidden.add(c.id);
      filtersChanged();
    };
    row.append(box, dot, text('span', baseName(c.top.path), 'name'), text('span', String(c.count), 'n'));
    list.append(row);
  }
  legend.append(head, list);
}
buildLegend();

// Theme: system / light / dark, remembered in this browser.
const themes = ['system', 'light', 'dark'];
const themeBtns = [...document.querySelectorAll('[data-theme-val]')];
let theme = 'system';
try { theme = themes.includes(localStorage.getItem('codegraph-theme')) ? localStorage.getItem('codegraph-theme') : 'system'; } catch (e) { /* storage blocked */ }
function refreshColors() {
  spriteTab = [[], [], [], []]; atlasTab = [null, null, null]; haloTab = []; shadowSprite = null; backdrop = null; floorGlow = null; colorCache.clear();
  bgVer++;
  drawCount = 0; // rebuilding sprites is a one-off spike
  readColors();
  legendDots.forEach(([dot, ci]) => { dot.style.background = colorOf(ci); });
  if (selected >= 0) document.querySelectorAll('#panel .dot').forEach(d => { d.style.background = colorOf(nodes[selected].ci); });
  paintBg(performance.now());
  invalidate();
}
function applyTheme() {
  if (theme === 'system') document.documentElement.removeAttribute('data-theme');
  else document.documentElement.setAttribute('data-theme', theme);
  themeBtns.forEach(b => b.setAttribute('aria-pressed', String(b.dataset.themeVal === theme)));
  refreshColors();
}
themeBtns.forEach(b => {
  b.onclick = () => {
    theme = themes.includes(b.dataset.themeVal) ? b.dataset.themeVal : 'system';
    try { localStorage.setItem('codegraph-theme', theme); } catch (e) { /* storage blocked */ }
    applyTheme();
  };
});

// Spin: the switch means "auto-rotate is allowed". Any interaction pauses the rotation and it
// resumes SPIN_DELAY after the last one (while the switch is on, in 3D, and nothing is pressed).
const spin = document.getElementById('spin');
const setSpin = on => { spin.checked = on; };
let spinWanted = !calm; // the user's own choice, restored when coming back to 3D
setSpin(spinWanted);
if (calm) { spin.disabled = true; spin.closest('label').classList.add('off'); } // reduced motion: nothing rotates, so the switch is inert
let lastInteract = -Infinity, resumeTimer = 0;
const spinOk = () => mode3d && spin.checked && !calm;
const pressed = () => !!drag || touches.size > 0;
function scheduleResume() { // wake the loop once the pause is over; costs nothing in between
  clearTimeout(resumeTimer); resumeTimer = 0;
  if (!spinOk()) return;
  const wait = SPIN_DELAY - (performance.now() - lastInteract);
  if (wait > 0) resumeTimer = setTimeout(() => { resumeTimer = 0; start(); }, wait + 30);
}
function interact() { lastInteract = performance.now(); scheduleResume(); }
spin.onchange = () => {
  spinWanted = spin.checked;
  if (spin.checked) lastInteract = -Infinity; // just switched on: start turning right away
  scheduleResume(); start();
};
function setMode(three) {
  if (three === mode3d) return;
  mode3d = three;
  document.getElementById('m3d').setAttribute('aria-pressed', String(three));
  document.getElementById('mflat').setAttribute('aria-pressed', String(!three));
  spin.disabled = !three;
  spin.parentElement.classList.toggle('off', !three);
  // Give the flat layout depth again with a deterministic nudge, then let it relax.
  if (mode3d) nodes.forEach(n => { n.z += ((n.i * 7919) % 100 - 50) * 0.2; });
  setSpin(mode3d && spinWanted);
  fitDirty = true;
  cancelGlide();
  userMoved = false;
  alpha = Math.max(alpha, 0.5);
  interact();
  invalidate();
}
document.getElementById('m3d').onclick = () => setMode(true);
document.getElementById('mflat').onclick = () => setMode(false);
document.getElementById('reset').onclick = () => { // back to the fitted view (keeps the spin choice and filters)
  cancelGlide();
  rot.x = 0.55; userMoved = false; fitDirty = true;
  fit(); interact(); invalidate();
};

const search = document.getElementById('search');
search.oninput = () => { filters.text = search.value.trim().toLowerCase(); filtersChanged(); };
search.onkeydown = e => {
  if (e.key !== 'Enter') return;
  const hit = nodes.find(n => n.vis && n.m);
  if (hit) { select(hit.i); center(hit.i); }
};
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && selected >= 0 && document.activeElement !== search) select(-1);
  const a = document.activeElement; // checkboxes and buttons don't take typed text, so `/` still works after clicking them
  const typing = !!a && (a.tagName === 'TEXTAREA' || a.tagName === 'SELECT' || (a.tagName === 'INPUT' && !/^(checkbox|radio|button)$/.test(a.type)));
  if (e.key === '/' && !typing && !e.ctrlKey && !e.metaKey && !e.altKey) { e.preventDefault(); search.focus(); }
});
// Anything the user does pauses the spin (it resumes by itself later).
document.addEventListener('keydown', interact, true);
document.addEventListener('pointerdown', interact, true);
document.addEventListener('pointerup', interact, true);
document.addEventListener('wheel', interact, { capture: true, passive: true });

