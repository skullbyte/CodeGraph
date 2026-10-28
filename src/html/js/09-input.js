// Pointer: drag rotates (3D), shift/right/middle-drag pans, two fingers pinch-zoom,
// click selects, hover shows a tooltip.
let drag = null, pinch = null, tipTimer = 0;
const touches = new Map();
const tip = document.getElementById('tip');
const hideTip = () => { clearTimeout(tipTimer); tip.classList.remove('show'); tip.setAttribute('aria-hidden', 'true'); };
function showTip(i, cx, cy) {
  const n = nodes[i];
  tip.replaceChildren(text('b', n.path), text('span', `${plural(n.symbols.length, 'symbol')}, ${plural(n.deg, 'link')}, community ${n.community}`));
  tip.classList.add('show');
  tip.removeAttribute('aria-hidden');
  const w = tip.offsetWidth, h = tip.offsetHeight;
  const x = cx + 14 + w > window.innerWidth - 8 ? cx - 14 - w : cx + 14;
  const y = Math.min(cy + 14, window.innerHeight - h - 8);
  tip.style.transform = '';
  tip.style.left = Math.max(8, x) + 'px'; tip.style.top = Math.max(8, y) + 'px';
}
const zoomTo = k => {
  k = clamp(k, 0.05, 20);
  const ratio = k / view.k;
  view.k = k; view.x *= ratio; view.y *= ratio;
};
canvas.oncontextmenu = e => e.preventDefault();
canvas.onpointerdown = e => {
  if (e.button === 1) e.preventDefault(); // no browser autoscroll
  hideTip();
  cancelGlide();
  if (touches.size >= 2) return; // a third finger is ignored
  touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
  canvas.setPointerCapture(e.pointerId);
  if (touches.size === 2) {
    const [p, q] = [...touches.values()];
    pinch = { d: Math.hypot(p.x - q.x, p.y - q.y) || 1, k: view.k };
    drag = null; userMoved = true;
    return;
  }
  drag = { x: e.clientX, y: e.clientY, moved: false, button: e.button, pan: !mode3d || e.shiftKey || e.button !== 0 };
  canvas.style.cursor = 'grabbing';
};
canvas.onpointermove = e => {
  if (touches.has(e.pointerId)) touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
  if (pinch && touches.size >= 2) {
    const [p, q] = [...touches.values()];
    zoomTo(pinch.k * Math.hypot(p.x - q.x, p.y - q.y) / pinch.d);
    invalidate();
    return;
  }
  if (!drag) {
    const box = canvas.getBoundingClientRect(), i = pick(e.clientX - box.left, e.clientY - box.top);
    canvas.style.cursor = i >= 0 ? 'pointer' : 'grab';
    hideTip();
    if (i >= 0 && e.pointerType === 'mouse') tipTimer = setTimeout(() => { // look again: the ball under the pointer may have moved
      const b = canvas.getBoundingClientRect(), j = pick(e.clientX - b.left, e.clientY - b.top);
      if (j >= 0) showTip(j, e.clientX, e.clientY);
    }, 120);
    return;
  }
  const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
  if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
  if (drag.moved && drag.pan) userMoved = true;
  if (drag.pan) { view.x += dx; view.y += dy; }
  else { rot.y += dx * 0.008; rot.x = clamp(rot.x + dy * 0.008, -1.5, 1.5); }
  drag.x = e.clientX; drag.y = e.clientY;
  invalidate();
};
canvas.onpointerleave = hideTip;
const release = e => {
  touches.delete(e.pointerId);
  if (touches.size < 2) pinch = null;
  canvas.style.cursor = 'grab';
};
canvas.onpointercancel = e => { release(e); drag = null; interact(); };
canvas.onpointerup = e => {
  const wasPinch = !!pinch;
  release(e);
  if (drag && !drag.moved && drag.button === 0 && !wasPinch) {
    const box = canvas.getBoundingClientRect();
    select(pick(e.clientX - box.left, e.clientY - box.top));
  }
  drag = null;
};
canvas.onwheel = e => {
  e.preventDefault();
  hideTip();
  cancelGlide();
  userMoved = true;
  const px = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 400 : 1); // lines or pages to pixels
  zoomTo(view.k * Math.exp(-px * 0.0015));
  invalidate();
};
// The view changes size with the window and also when the side panel grows or shrinks (phones).
// The canvases follow at once; the (costly) nebula image is rebuilt only after the size stops changing.
function onResize() {
  resize();
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => { resizeTimer = 0; bgSig[3] = -1; paintBg(performance.now()); }, 150);
}
if (window.ResizeObserver) new ResizeObserver(onResize).observe(document.getElementById('view'));
else window.onresize = onResize;
// A new device-pixel ratio (window moved to another screen, browser zoom) does not resize the view, so it needs its
// own watch. The query names the current ratio and fires once when it changes; then we watch the new ratio.
function watchPixelRatio() {
  const off = onMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`, () => { off(); onResize(); watchPixelRatio(); });
}
watchPixelRatio();
onMedia('(prefers-color-scheme: dark)', () => { if (theme === 'system') refreshColors(); }); // an explicit light/dark choice ignores the system
document.addEventListener('visibilitychange', () => { if (!document.hidden) { lastFrame = 0; start(); } });

