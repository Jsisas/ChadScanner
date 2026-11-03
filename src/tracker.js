// ChadScanner -> tarkovtracker.org bridge. Injected into every main-frame document; acts only on tarkovtracker.org.
// Selectors match the site in English (aria-labels); calls run one after another, in order.
(() => {
  if (window.__chad || !/(^|\.)tarkovtracker\.org$/.test(location.hostname)) return;
  const post = (o) => window.ipc && window.ipc.postMessage(JSON.stringify(o));
  const toast = (title, body, warn) => post({ toast: { title, body, warn } });
  const log = (text) => post({ log: text });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  async function until(fn, ms) {
    const end = Date.now() + ms;
    for (;;) {
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

  async function open(id, name) {
    if (modalFor(id)) return true;
    raised = [];
    const r = await until(router, 15000);
    if (!r) return log('tracker: page not ready'), false;
    const other = itemModal();
    if (other) {
      other.querySelector('button[aria-label="Close"]')?.click();
      await until(() => !itemModal(), 2000);
    }
    const route = r.currentRoute.value;
    if (route.path !== '/needed-items' || route.query.q !== name) await r.replace({ path: '/needed-items', query: { q: name } });
    const combined = await until(() => document.querySelector('button[aria-label="Combined view"]'), 8000);
    if (combined && combined.getAttribute('aria-pressed') === 'false') combined.click();
    const c = await until(() => card(id, name), 8000);
    if (!c) return log('tracker: ' + name + ' not in needed items'), false;
    c.click();
    const ok = !!(await until(() => modalFor(id), 5000));
    log('tracker: ' + name + (ok ? ' opened' : ' did not open'));
    return ok;
  }

  // Up: FIR first; Non-FIR once FIR is full (only with `overfill`) or when the item needs no FIR.
  // Down: the newest raised kind, else Non-FIR, else FIR.
  async function step(id, name, up, overfill) {
    if (!(await open(id, name))) return toast(name, 'not in your TarkovTracker needed items', true);
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
    await Promise.resolve(); // Vue re-renders on the microtask queue
    toast(name, counts(m), false);
  }

  function smartFill() {
    const m = itemModal();
    const b = m && m.querySelector('[class*="i-mdi:auto-fix"]')?.closest('button');
    if (!b) return log('tracker: no item open for Smart Fill');
    if (b.disabled) return;
    b.click();
    log('tracker: Smart Fill ' + counts(m));
  }

  let queue = Promise.resolve();
  const run = (f) => {
    queue = queue.then(f).catch((e) => log('tracker: ' + e));
  };
  window.__chad = {
    open: (id, name) => run(() => open(id, name)),
    step: (id, name, up, overfill) => run(() => step(id, name, up, overfill)),
    smartFill: () => run(smartFill),
  };
})();
