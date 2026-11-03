// ChadScanner -> tarkovtracker.org bridge. Injected into every main-frame document; acts only on tarkovtracker.org.
// Selectors match the site in English (aria-labels); calls run one after another, in order.
(() => {
  if (window.__chad || !/(^|\.)tarkovtracker\.org$/.test(location.hostname)) return;
  const post = (o) => window.ipc && window.ipc.postMessage(JSON.stringify(o));
  const toast = (title, body, warn) => post({ toast: { title, body, warn } });
  const log = (text) => post({ log: text });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // `g` is the generation of the scan that started the wait; a newer scan (higher `gen`) aborts it with null.
  async function until(fn, ms, g) {
    const end = Date.now() + ms;
    for (;;) {
      if (g != null && g !== gen) return null;
      const v = fn();
      if (v || Date.now() > end) return v || null;
      await sleep(100);
    }
  }
  const CARD = '[role=button][aria-label^="View details for"]';
  const itemModal = () =>
    [...document.querySelectorAll('[role=dialog][data-state=open]')].find((d) => d.querySelector('button[aria-label$="FIR count"]')) || null;
  const modalFor = (id) => {
    const d = itemModal();
    return d && d.querySelector('img[src*="/' + id + '-"]') ? d : null;
  };
  const router = () => document.querySelector('#__nuxt')?.__vue_app__?.config.globalProperties.$router || null;
  const card = (id, name) =>
    document.querySelector(CARD + ' img[src*="/' + id + '-"]')?.closest(CARD) ||
    [...document.querySelectorAll(CARD)].find((c) => c.getAttribute('aria-label') === 'View details for ' + name) ||
    null;
  const empty = () => [...document.querySelectorAll('[aria-live=polite]')].some((d) => d.textContent.trim() === 'No items match your search.');
  const button = (m, dir, kind) => m.querySelector('button[aria-label="' + dir + ' ' + kind + ' count"]');
  const usable = (b) => !!b && !b.disabled;
  const counts = (m) =>
    ['FIR', 'Non-FIR']
      .map((kind) => {
        const inc = button(m, 'Increase', kind);
        const t = inc && inc.parentElement.parentElement.textContent.match(/([\d,.\s]+)\/([\d,.\s]+)/);
        return t ? kind + ' ' + t[1].replace(/\D/g, '') + '/' + t[2].replace(/\D/g, '') : null;
      })
      .filter(Boolean)
      .join(' · ');

  // Kinds Arrow Up raised in the open modal, newest last; Arrow Down undoes the newest first.
  let raised = [];

  // `g`: generation of the scan that asked for this open; once a newer scan bumps `gen` the open stops silently.
  // Arrow steps pass null: they are never cancelled, so no key press is lost.
  async function open(id, name, g) {
    const stale = () => g != null && g !== gen;
    if (modalFor(id)) return true;
    if (stale()) return false;
    raised = [];
    const r = await until(router, 15000, g);
    if (stale()) return false;
    if (!r) return log('tracker: page not ready'), false;
    const other = itemModal();
    if (other) {
      other.querySelector('button[aria-label="Close"]')?.click();
      await until(() => !itemModal(), 2000, g);
      if (stale()) return false;
    }
    const route = r.currentRoute.value;
    if (route.path !== '/needed-items' || route.query.q !== name) await r.replace({ path: '/needed-items', query: { q: name } });
    if (stale()) return false;
    const combined = await until(() => document.querySelector('button[aria-label="Combined view"]'), 8000, g);
    if (stale()) return false;
    if (combined && combined.getAttribute('aria-pressed') === 'false') combined.click();
    let emptySince = 0;
    const c = await until(() => {
      const found = card(id, name);
      if (found) return found;
      if (!empty()) return (emptySince = 0), null;
      emptySince ||= Date.now();
      return Date.now() - emptySince >= 400 ? 'none' : null;
    }, 8000, g);
    if (stale()) return false;
    if (!c || c === 'none') return log('tracker: ' + name + ' not in needed items'), false;
    c.click();
    const ok = !!(await until(() => modalFor(id), 5000, g));
    if (stale()) return false;
    log('tracker: ' + name + (ok ? ' opened' : ' did not open'));
    return ok;
  }

  // Up: FIR first; Non-FIR once FIR is full (only with `overfill`) or when the item needs no FIR.
  // Down: the newest raised kind, else Non-FIR, else FIR.
  async function step(id, name, up, overfill) {
    if (!(await open(id, name, null))) return toast(name, 'not in your TarkovTracker needed items', true);
    const m = modalFor(id);
    let kind = null;
    if (up) {
      const fir = button(m, 'Increase', 'FIR');
      if (usable(fir)) kind = 'FIR';
      else if ((!fir || overfill) && usable(button(m, 'Increase', 'Non-FIR'))) kind = 'Non-FIR';
    } else {
      kind = [...raised].reverse().concat(['Non-FIR', 'FIR']).find((k) => usable(button(m, 'Decrease', k))) || null;
    }
    if (!kind) return toast(name, (up ? 'nothing left to add · ' : 'nothing to remove · ') + counts(m), true);
    button(m, up ? 'Increase' : 'Decrease', kind).click();
    if (up) raised.push(kind);
    else if (raised.lastIndexOf(kind) >= 0) raised.splice(raised.lastIndexOf(kind), 1);
    await sleep(0); // Vue re-renders on the microtask queue; a macrotask later the Smart Fill button's state is current
    smartFill();
    toast(name, counts(m), false);
    postProgress();
  }

  // Writes the modal's collected totals into the objective/hideout counts. The site disables Smart Fill at 0/0,
  // so a drop to zero goes through Reset, which zeroes every need of the item (the same distribution).
  function smartFill() {
    const m = itemModal();
    const b = m && m.querySelector('[class*="i-mdi:auto-fix"]')?.closest('button');
    if (!b) return log('tracker: no item open for Smart Fill');
    if (b.disabled) {
      const reset = b.parentElement.querySelector('[class*="i-mdi:refresh"]')?.closest('button');
      if (!usable(reset)) return;
      reset.click();
      return log('tracker: Smart Fill at 0/0 -> Reset');
    }
    b.click();
    log('tracker: Smart Fill ' + counts(m));
  }

  // Reports the site's own progress store (current game mode) so the card can redraw without waiting for the API.
  const tarkovState = () => {
    const s = document.querySelector('#__nuxt')?.__vue_app__?.config.globalProperties.$pinia?.state.value.swapTarkov;
    return s ? { mode: s.currentGameMode, d: s[s.currentGameMode] || {} } : null;
  };
  function postProgress() {
    const s = tarkovState();
    if (!s) return log('tracker: no progress store');
    const pick = (m) =>
      Object.fromEntries(Object.entries(m || {}).map(([id, e]) => [id, { count: Number(e?.count) || 0, complete: !!e?.complete }]));
    post({ progress: { mode: s.mode, objectives: pick(s.d.taskObjectives), parts: pick(s.d.hideoutParts) } });
  }

  let gen = 0; // scan generation: each scan's open() bumps it, which aborts older scans' opens
  let queue = Promise.resolve();
  const run = (f) => {
    queue = queue.then(f).catch((e) => log('tracker: ' + e));
  };
  // Bumping `gen` at call time (not when the queued job starts) is what lets a newer scan cancel older opens.
  window.__chad = {
    open: (id, name) => {
      const g = ++gen;
      run(() => open(id, name, g));
    },
    step: (id, name, up, overfill) => run(() => step(id, name, up, overfill)),
  };
})();
