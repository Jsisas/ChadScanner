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

  // ChadScanner Fill: sets every target to its entry of `counts` (the app's from-scratch split of the popup's
  // collected totals in card order). Paired finds are only raised: a find can legitimately exceed the hand-in.
  async function fill(id, name, targets, counts, firLeft, nonFirLeft) {
    // The click came from the open popup; never open one here.
    if (!modalFor(id)) return toast(name, 'popup closed', true);
    const store = tarkovStore();
    if (!store || !tarkovState()) return log('tracker: no progress store'), toast(name, 'TarkovTracker progress not loaded', true);
    targets.forEach((t, i) => {
      const n = counts[i];
      if (cur(t, t.id) !== n) write(store, t, t.id, n);
      if (t.find && cur(t, t.find) < n) write(store, t, t.find, n);
    });
    raised = [];
    const parts = targets
      .map((t, i) => [t, counts[i]])
      .filter(([, n]) => n > 0)
      .map(([t, n]) => t.label + ' ' + n + '/' + t.required + (t.fir ? ' FIR' : ''));
    let body = parts.length ? parts.join(' · ') : 'all needs set to 0';
    if (firLeft) body += ' · ' + firLeft + ' FIR unused';
    if (nonFirLeft) body += ' · ' + nonFirLeft + ' non-FIR unused';
    toast(name, body, firLeft + nonFirLeft > 0);
    log('tracker: fill ' + name);
    postProgress();
  }

  // Reports the site's own progress store (current game mode) so the card can redraw without waiting for the API.
  const tarkovState = () => {
    const s = document.querySelector('#__nuxt')?.__vue_app__?.config.globalProperties.$pinia?.state.value.swapTarkov;
    return s ? { mode: s.currentGameMode, d: s[s.currentGameMode] || {} } : null;
  };
  // Current count of objective / hideout part `oid` (live read of the site's store, so it reflects each write).
  const cur = (t, oid) => {
    const d = tarkovState().d;
    const e = (t.kind === 'task' ? d.taskObjectives : d.hideoutParts)?.[oid];
    const c = Number(e?.count) || 0;
    return e?.complete ? Math.max(c, t.required) : c;
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
    fill: (id, name, targets, counts, firLeft, nonFirLeft) => run(() => fill(id, name, targets, counts, firLeft, nonFirLeft)),
  };

  // ChadScanner Fill button next to Smart Fill in the item popup. The popup's counters are component refs, so
  // the click reads the visible totals and the app answers with per-target counts (`fill`).
  const modalItemId = (m) => {
    for (const img of m.querySelectorAll('img')) {
      const hit = /\/([0-9a-f]{24})-[^/]*$/.exec(img.getAttribute('src') || '');
      if (hit) return hit[1];
    }
    return null;
  };
  // Collected count of `kind` ('FIR' / 'Non-FIR'): the number between the row's - and + buttons. The site hides
  // the row when nothing of that kind is needed, so a missing one is 0.
  const collected = (m, kind) => {
    const box = m.querySelector('button[aria-label="Increase ' + kind + ' count"]')?.previousElementSibling;
    if (!box) return 0;
    const input = box.querySelector('input');
    return Number(input ? input.value : box.textContent.replace(/\D/g, '')) || 0;
  };
  const missingSmartFill = new WeakSet();
  function ensureButton() {
    const m = itemModal();
    if (!m || m.querySelector('[data-chad-fill]')) return;
    const sf = m.querySelector('[class*="i-mdi:auto-fix"]')?.closest('button');
    if (!sf) {
      if (!missingSmartFill.has(m)) (missingSmartFill.add(m), log('tracker: no Smart Fill button'));
      return;
    }
    const b = document.createElement('button');
    b.type = 'button';
    b.className = sf.className;
    b.textContent = 'ChadScanner Fill';
    b.dataset.chadFill = '1';
    b.title = "Distribute the collected counts in ChadScanner's card order";
    b.onclick = () => {
      const id = modalItemId(m);
      if (!id) return log('tracker: fill: no item id');
      post({ fill: { id, fir: collected(m, 'FIR'), nonFir: collected(m, 'Non-FIR') } });
    };
    sf.after(b);
  }
  const watch = () => new MutationObserver(ensureButton).observe(document.body, { childList: true, subtree: true });
  document.readyState === 'loading' ? document.addEventListener('DOMContentLoaded', watch) : watch();
})();
