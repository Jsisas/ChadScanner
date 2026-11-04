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

  // Target ids Arrow Up raised for the open item, newest last; Arrow Down undoes the newest first.
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

  const tarkovStore = () => document.querySelector('#__nuxt')?.__vue_app__?.config.globalProperties.$pinia?._s?.get('swapTarkov') || null;

  // Sets one objective / hideout-part count the way the modal's own +/- rows do. Never marks an objective
  // complete (the site does that itself, like Smart Fill); hideout parts flip complete at their requirement.
  function write(store, t, oid, n) {
    if (t.kind === 'task') {
      store.setObjectiveCount(oid, n);
      if (n < t.required && store.isTaskObjectiveComplete(oid)) store.setTaskObjectiveUncomplete(oid);
    } else {
      store.setHideoutPartCount(oid, n);
      const done = n >= t.required;
      if (done !== store.isHideoutPartComplete(oid)) (done ? store.setHideoutPartComplete(oid) : store.setHideoutPartUncomplete(oid));
    }
  }

  // One Arrow press for the item. `targets` is the card's fill order (Up: already filtered by the app).
  // Up: +1 on the first target still short. Down: -1 on the newest raised target, else the last one with a count.
  async function step(id, name, up, targets) {
    if (!(await open(id, name, null))) return toast(name, 'not in your TarkovTracker needed items', true);
    const store = tarkovStore();
    if (!store || !tarkovState()) return log('tracker: no progress store'), toast(name, 'TarkovTracker progress not loaded', true);
    // Live read of the site's store, so it reflects each write.
    const cur = (t, oid) => {
      const d = tarkovState().d;
      const e = (t.kind === 'task' ? d.taskObjectives : d.hideoutParts)?.[oid];
      const c = Number(e?.count) || 0;
      return e?.complete ? Math.max(c, t.required) : c;
    };
    let t;
    if (up) {
      t = targets.find((x) => Math.min(cur(x, x.id), x.find ? cur(x, x.find) : Infinity) < x.required);
      if (!t) return toast(name, 'nothing left to add', true);
      const n = Math.min(cur(t, t.id) + 1, t.required);
      write(store, t, t.id, n);
      if (t.find && cur(t, t.find) < n) write(store, t, t.find, n);
      raised.push(t.id);
    } else {
      t =
        [...raised]
          .reverse()
          .map((r) => targets.find((x) => x.id === r))
          .find((x) => x && cur(x, x.id) > 0) || [...targets].reverse().find((x) => cur(x, x.id) > 0);
      if (!t) return toast(name, 'nothing to remove', true);
      const n = cur(t, t.id) - 1;
      write(store, t, t.id, n);
      if (t.find && cur(t, t.find) > n) write(store, t, t.find, n);
      const i = raised.lastIndexOf(t.id);
      if (i >= 0) raised.splice(i, 1);
    }
    toast(name, t.label + ' ' + cur(t, t.id) + '/' + t.required + (t.fir ? ' FIR' : ''), false);
    log('tracker: ' + (up ? '+1 ' : '-1 ') + t.label + ' (' + t.id + ')');
    postProgress();
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
    step: (id, name, up, targets) => run(() => step(id, name, up, targets)),
  };
})();
