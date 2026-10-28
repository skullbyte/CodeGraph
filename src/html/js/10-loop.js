// ---- Render loop ------------------------------------------------------------------------------
// The graph canvas is repainted only when `dirty` is set (rotation, zoom, pan, filters, selection, layout
// still moving, intro, resize, theme). The backdrop has its own cheap repaint. When nothing moves the loop
// sleeps: in the dark theme it ticks 20 times a second just for the star drift; otherwise it stops completely
// (also in calm mode, in a hidden tab, and in low quality), and timers wake it up again (spin resume).
let lastFrame = 0, raf = 0, idleTimer = 0;
const SLICE_MS = 6; // time budget for layout steps in one frame
function invalidate() { dirty = true; start(); }
function frame(ts) {
  raf = 0;
  const now = ts || performance.now();
  const dt = lastFrame ? Math.min((now - lastFrame) / 1000, 0.1) : 1 / 60; // seconds; keeps spin speed equal on 60/120 Hz
  lastFrame = now;
  let stepped = false;
  if (alpha > 0.02) { // layout: up to 4 steps, but stop once the time budget is spent
    const s0 = performance.now();
    for (let i = 0; i < 4 && alpha > 0.02; i++) {
      step(); stepped = true;
      const spent = performance.now() - s0;
      if (spent + spent / (i + 1) > SLICE_MS) break; // the next step would not fit in the budget
    }
    dirty = true;
  }
  const spinning = spinOk() && !pressed() && now - lastInteract >= SPIN_DELAY;
  if (spinning) { rot.y += 0.12 * dt; dirty = true; }
  if (!userMoved && (stepped || fitDirty)) fit();
  const intro = !calm && now - born < 1300;
  if (intro) dirty = true;
  if (dirty) draw();
  paintBg(now);
  const live = gliding || (ping && now - ping.t0 < 470);
  if (alpha > 0.02 || spinning || intro || live) { raf = requestAnimationFrame(frame); return; }
  if (ambient()) { // only the star drift is left
    idleTimer = setTimeout(() => { idleTimer = 0; raf = requestAnimationFrame(frame); }, 50);
    return;
  }
  lastFrame = 0;
  scheduleResume();
}
function start() {
  if (raf) return;
  if (idleTimer) { clearTimeout(idleTimer); idleTimer = 0; }
  raf = requestAnimationFrame(frame);
}
refreshVis();
select(-1);
applyTheme();
resize();
start();
