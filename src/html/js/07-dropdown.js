// ---- Dropdown: a button that opens a themed listbox (keyboard: Enter/Space/arrows, Esc, type-ahead) ------
const dropdowns = [];
function makeDropdown(host, { id, label, iconName, allLabel, options, onChange }) {
  const wrap = document.createElement('div'), btn = document.createElement('button'), menu = document.createElement('div');
  wrap.className = 'dd'; btn.type = 'button'; btn.className = 'ddbtn'; btn.id = id;
  btn.setAttribute('aria-haspopup', 'listbox'); btn.setAttribute('aria-expanded', 'false'); btn.setAttribute('aria-controls', id + '-list');
  const lbl = text('span', allLabel, 'lbl'), chev = icon('chev');
  chev.setAttribute('class', 'ic dchev');
  btn.append(icon(iconName), lbl, chev);
  menu.id = id + '-list'; menu.className = 'ddmenu'; menu.setAttribute('role', 'listbox'); menu.setAttribute('aria-label', label); menu.tabIndex = -1; menu.hidden = true;
  const all = [{ value: '', label: allLabel, count: nodes.length }, ...options];
  const els = all.map((o, k) => {
    const el = document.createElement('div');
    el.className = 'ddopt'; el.id = `${id}-o${k}`; el.setAttribute('role', 'option'); el.setAttribute('aria-selected', 'false');
    el.append(icon('check'), text('span', o.label, 'txt'), text('span', String(o.count), 'num'));
    el.firstChild.setAttribute('class', 'ic tick');
    el.onmousemove = () => mark(k, false);
    el.onclick = () => { choose(k); closeMenu(true); };
    menu.append(el);
    if (k === 0 && all.length > 1) menu.append(Object.assign(document.createElement('div'), { className: 'ddsep' }));
    return el;
  });
  menu.querySelectorAll('.ddsep').forEach(s => s.setAttribute('aria-hidden', 'true')); // a listbox holds options only
  let value = '', cur = 0, isOpen = false, typed = '', typedAt = 0;
  const indexOf = v => Math.max(0, all.findIndex(o => o.value === v));
  function mark(k, scroll = true) {
    cur = clamp(k, 0, all.length - 1);
    els.forEach((el, j) => el.classList.toggle('cur', j === cur));
    menu.setAttribute('aria-activedescendant', els[cur].id);
    if (!scroll) return; // hovering must not move the list under the pointer
    const el = els[cur]; // scroll only the menu itself, never the page
    if (el.offsetTop < menu.scrollTop) menu.scrollTop = el.offsetTop;
    else if (el.offsetTop + el.offsetHeight > menu.scrollTop + menu.clientHeight) menu.scrollTop = el.offsetTop + el.offsetHeight - menu.clientHeight;
  }
  function setValue(v) {
    value = v;
    const k = indexOf(v);
    els.forEach((el, j) => el.setAttribute('aria-selected', String(j === k)));
    lbl.textContent = all[k].label;
    btn.classList.toggle('active', v !== '');
    btn.title = `${label}: ${all[k].label}`;
    btn.setAttribute('aria-label', `${label}: ${all[k].label}`);
  }
  function choose(k) {
    const v = all[clamp(k, 0, all.length - 1)].value;
    if (v === value) return;
    setValue(v); onChange(v);
  }
  function openMenu() {
    if (isOpen) return;
    dropdowns.forEach(d => d.close());
    isOpen = true; menu.hidden = false; btn.setAttribute('aria-expanded', 'true');
    menu.style.left = ''; menu.style.right = '';
    if (menu.getBoundingClientRect().right > document.documentElement.clientWidth - 4) { menu.style.left = 'auto'; menu.style.right = '0'; } // keep it on screen
    mark(indexOf(value));
    menu.focus({ preventScroll: true });
  }
  function closeMenu(refocus) {
    typed = ''; // a reopened menu starts a fresh type-ahead
    if (!isOpen) return;
    isOpen = false; menu.hidden = true; menu.style.left = ''; menu.style.right = ''; btn.setAttribute('aria-expanded', 'false');
    if (refocus) btn.focus();
  }
  btn.onclick = () => (isOpen ? closeMenu(true) : openMenu());
  btn.onkeydown = e => { if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); openMenu(); } };
  menu.onkeydown = e => {
    const key = e.key;
    if (key === 'ArrowDown') mark(cur + 1);
    else if (key === 'ArrowUp') mark(cur - 1);
    else if (key === 'Home') mark(0);
    else if (key === 'End') mark(all.length - 1);
    else if (key === 'Enter' || (key === ' ' && !(typed && performance.now() - typedAt <= 700))) { choose(cur); closeMenu(true); }
    else if (key === 'Escape') closeMenu(true);
    else if (key === 'Tab') { closeMenu(false); return; }
    else if (key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) { // type-ahead: jump to the next label starting with what was typed
      const now = performance.now();
      typed = now - typedAt > 700 ? key.toLowerCase() : typed + key.toLowerCase(); typedAt = now;
      const find = (needle, from) => { for (let s = 0; s < all.length; s++) { const k = (from + s) % all.length; if (all[k].label.toLowerCase().startsWith(needle)) return k; } return -1; };
      let k = find(typed, typed.length === 1 ? cur + 1 : cur); // one letter: the next label that starts with it
      if (k < 0 && [...typed].every(c => c === typed[0])) k = find(typed[0], cur + 1); // "ff" with no label starting "ff": cycle on "f"
      if (k >= 0) mark(k);
    } else return;
    e.preventDefault(); e.stopPropagation(); // keep Esc and `/` from reaching the page shortcuts while the list is open
  };
  document.addEventListener('pointerdown', e => { if (isOpen && !wrap.contains(e.target)) closeMenu(false); });
  wrap.append(btn, menu);
  host.append(wrap);
  setValue('');
  const api = { set: v => setValue(v), close: () => closeMenu(false), el: btn };
  dropdowns.push(api);
  return api;
}

const sorted = set => [...set].sort();
function countBy(keysOf) {
  const m = new Map();
  for (const n of nodes) for (const k of new Set(keysOf(n))) m.set(k, (m.get(k) || 0) + 1);
  return m;
}
const folderCounts = countBy(n => [n.folder]), kindCounts = countBy(n => n.symbols.map(s => s.kind));
makeDropdown(document.getElementById('folderHost'), {
  id: 'folder', label: 'Filter by folder', iconName: 'folder', allLabel: 'All folders',
  options: sorted(folderCounts.keys()).map(v => ({ value: v, label: v === '.' ? '(root)' : v, count: folderCounts.get(v) })),
  onChange: v => { filters.folder = v; filtersChanged(); },
});
makeDropdown(document.getElementById('kindHost'), {
  id: 'kind', label: 'Filter by symbol kind', iconName: 'tag', allLabel: 'All symbol kinds',
  options: [...kindCounts.keys()].map(v => ({ value: v, label: kindLabel(v), count: kindCounts.get(v) })).sort((a, b) => a.label.localeCompare(b.label)),
  onChange: v => { filters.kind = v; filtersChanged(); },
});

