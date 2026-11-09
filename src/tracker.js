// ChadScanner -> tarkovtracker.org bridge. Injected into every main-frame document; acts only on tarkovtracker.org.
// Selectors match the site in English (aria-labels); calls run one after another, in order.
(() => {
  if (window.__chad || !/(^|\.)tarkovtracker\.org$/.test(location.hostname)) return;
  const post = (o) => window.ipc && window.ipc.postMessage(JSON.stringify(o));
  const toast = (title, body, warn) => post({ toast: { title, body, warn } });
  const log = (text) => post({ log: text });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // Raid map on the site's own Leaflet map (/tasks/maps). Leaflet's UMD build assigns window.L when its chunk loads;
  // the trap hooks L.Map then, before the site creates a map, so every map instance is known here.
  const leafletMaps = new Set();
  let Lf = window.L;
  const hookLeaflet = (v) => {
    if (!v || !v.Map || v.Map.__chad) return;
    v.Map.__chad = true;
    v.Map.addInitHook(function () {
      leafletMaps.add(this);
      this.whenReady(async () => {
        applyRaid(this);
        const t = await until(() => toolbarOf(this), 5000, null);
        if (t) refreshToolbar(t);
      });
      this.on('layeradd', () => scheduleSweep(this));
      this.on('unload', () => leafletMaps.delete(this));
    });
  };
  hookLeaflet(Lf);
  Object.defineProperty(window, 'L', { configurable: true, get: () => Lf, set: (v) => { Lf = v; hookLeaflet(v); } });
  const liveMaps = () => [...leafletMaps].filter((m) => m.getContainer().isConnected);

  // {mapId, siteMap, fs: full screen opened, rotation, keep: extract names | null, side: 'pmc' | 'scav' | null (null: not proven, shown as PMC), here: {pos, yaw} | null, back: route to restore at raid end}
  let raid = null;

  // Hides every extract whose name is not in `raid.keep`; unhides everything when there is no raid. The site draws
  // each extract as a circle-marker dot plus a badge marker at the same spot, and rebuilds them on toggles.
  const norm = (s) => s.trim().toLowerCase();
  const key = (ll) => ll.lat.toFixed(2) + ',' + ll.lng.toFixed(2);
  const isBadge = (l) => l instanceof Lf.Marker && l.options.icon && l.options.icon.options.className === 'extract-marker';
  const badgeName = (l) => {
    const h = l.options.icon.options.html;
    if (typeof h === 'string') return (/title="([^"]*)"/.exec(h) || [])[1] || '';
    if (!h || !h.getAttribute) return '';
    return h.getAttribute('title') || h.querySelector('[title]')?.getAttribute('title') || '';
  };
  const isDot = (l) => l instanceof Lf.CircleMarker && l.options.radius === 3 && l.options.interactive === false;
  function sweep(map) {
    const badges = [];
    const dots = [];
    map.eachLayer((l) => {
      if (isBadge(l)) badges.push(l);
      else if (isDot(l)) dots.push(l);
    });
    const hide = new Set();
    if (raid && raid.keep && chadOn) {
      const keep = new Set(raid.keep.map(norm));
      const kept = badges.filter((b) => keep.has(norm(badgeName(b))));
      if (kept.length) {
        // A pmc and a scav extract can share one spot: kept wins.
        const keptAt = new Set(kept.map((b) => key(b.getLatLng())));
        badges.forEach((b) => {
          const k = key(b.getLatLng());
          if (!keep.has(norm(badgeName(b))) && !keptAt.has(k)) hide.add(k);
        });
      } else if (badges.length && !raid.warned) {
        raid.warned = true;
        log('tracker: no extract on the map matches ' + raid.keep.join(', '));
      }
    }
    badges.forEach((l) => {
      const el = l.getElement();
      if (el) el.style.display = hide.has(key(l.getLatLng())) ? 'none' : '';
    });
    dots.forEach((l) => {
      const h = hide.has(key(l.getLatLng()));
      const o = h ? 0 : 1;
      if (l.options.opacity !== o || l.options.fillOpacity !== o) l.setStyle({ opacity: o, fillOpacity: o });
    });
  }
  const sweepPending = new WeakSet();
  function scheduleSweep(map) {
    if (sweepPending.has(map)) return;
    sweepPending.add(map);
    setTimeout(() => {
      sweepPending.delete(map);
      sweep(map);
    }, 0);
  }

  // Chad mode: ChadScanner's layer defaults with the site's own toggles shown off; any site toggle leaves it.
  // Starts on for every document and every new raid.
  let chadOn = true;
  let applying = false; // our own toggle clicks pass the toolbar's capture listener
  const LAYERS = ['PMC', 'Scav', 'PMC Spawns'];
  const PRESSED = 'border-primary-400/60 bg-primary-500/15 text-primary-100 light:text-primary-900'.split(' ');
  const UNPRESSED = 'border-transparent text-surface-300/60 hover:bg-white/5 hover:text-surface-100'.split(' ');
  const baseClass = (c) => c.split(/\s+/).filter((t) => t && !PRESSED.includes(t) && !UNPRESSED.includes(t)).join(' ');
  const layerButton = (toolbar, label) =>
    [...toolbar.querySelectorAll('button[aria-pressed]:not([data-chad-layers])')].find((x) => x.textContent.trim() === label);
  const toolbarOf = (map) => {
    for (let e = map.getContainer().parentElement; e; e = e.parentElement) {
      const t = e.querySelector('[data-testid="map-toolbar"]');
      if (t) return t;
    }
    return null;
  };
  // want: label -> bool
  function setLayers(toolbar, want) {
    applying = true;
    try {
      for (const label of LAYERS) {
        const b = layerButton(toolbar, label);
        if (b && b.getAttribute('aria-pressed') !== String(want[label])) b.click();
      }
    } finally {
      applying = false;
    }
  }
  // In a raid the side's own layer is on and the other side's off; until the exfil panel proves Scav the raid counts as
  // PMC (Scav panels list Scav-only extracts, PMC panels often only shared ones). Outside a raid both are on. PMC spawns always on.
  const chadLayers = () => {
    const side = raid ? raid.side || 'pmc' : null;
    return { PMC: side !== 'scav', Scav: side !== 'pmc', 'PMC Spawns': true };
  };
  // Puts `toolbar` in the current mode: layers (Chad on only), the Chad button's pressed look, and the
  // `data-chad-on` marker that makes pressed site toggles look unpressed.
  function refreshToolbar(toolbar) {
    if (chadOn) setLayers(toolbar, chadLayers());
    toolbar.toggleAttribute('data-chad-on', chadOn);
    const b = toolbar.querySelector('[data-chad-layers]');
    if (b) {
      b.className = baseClass(b.className) + ' ' + (chadOn ? PRESSED : UNPRESSED).join(' ');
      b.setAttribute('aria-pressed', String(chadOn));
    }
  }
  const refreshAll = () => {
    document.querySelectorAll('[data-testid="map-toolbar"]').forEach(refreshToolbar);
    if (raid) raid.warned = false;
    liveMaps().forEach(sweep);
  };
  // Styles the site's pressed toggles like unpressed ones while a toolbar has `data-chad-on`; the colours are read
  // from a hidden unpressed copy of the PMC button so they follow the site's theme.
  function ensureChadStyle(toolbar) {
    if (document.getElementById('chad-layers-style')) return;
    const pmc = layerButton(toolbar, 'PMC');
    if (!pmc) return;
    const probe = document.createElement('button');
    probe.className = baseClass(pmc.className) + ' ' + UNPRESSED.join(' ');
    probe.style.cssText = 'position:absolute;visibility:hidden;pointer-events:none';
    toolbar.appendChild(probe);
    const cs = getComputedStyle(probe);
    const [c, bc, bg] = [cs.color, cs.borderTopColor, cs.backgroundColor];
    probe.remove();
    const s = document.createElement('style');
    s.id = 'chad-layers-style';
    s.textContent = `[data-chad-on] button[aria-pressed="true"]:not([data-chad-layers]){color:${c}!important;border-color:${bc}!important;background-color:${bg}!important}`;
    document.head.appendChild(s);
  }

  // The player's arrow: one marker per map, turned by the screenshot yaw plus the map's rotation.
  const arrows = new WeakMap();
  function drawHere(map, pan) {
    if (!map._loaded) return;
    if (!raid || !raid.here) {
      const a = arrows.get(map);
      if (a) {
        a.remove();
        arrows.delete(map);
      }
      return;
    }
    if (!document.getElementById('chad-me-style')) {
      const s = document.createElement('style');
      s.id = 'chad-me-style';
      s.textContent =
        '.chad-me-wrap{background:none;border:none}' +
        '.chad-me{width:0;height:0;border-left:9px solid transparent;border-right:9px solid transparent;border-bottom:22px solid #ffd400;filter:drop-shadow(0 0 2px #000);transform-origin:50% 50%}';
      document.head.appendChild(s);
    }
    let r = raid.rotation || 0;
    if (r === 90 || r === 270) r += 180; // tarkov.dev's addRotation rule
    const icon = Lf.divIcon({
      className: 'chad-me-wrap',
      html: '<div class="chad-me" style="rotate:' + (raid.here.yaw + r) + 'deg"></div>',
      iconSize: [18, 22],
      iconAnchor: [9, 11],
    });
    let a = arrows.get(map);
    if (a) a.setLatLng(raid.here.pos).setIcon(icon);
    else {
      a = Lf.marker(raid.here.pos, { icon, zIndexOffset: 2000, interactive: false }).addTo(map);
      arrows.set(map, a);
    }
    // Pan only when the arrow is off-screen; an arrow already in view must not move the map.
    if (pan) map.panInside(raid.here.pos, { padding: [40, 40] });
  }
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
    exitFullscreen();
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

  // Chad Fill: sets every target to its entry of `counts`, the app's from-scratch split of the popup's
  // Collected totals over the popup's rows in card order. Paired finds are only raised: a find can legitimately
  // exceed the hand-in.
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
    post({
      progress: { mode: s.mode, objectives: pick(s.d.taskObjectives), parts: pick(s.d.hideoutParts), modules: pick(s.d.hideoutModules) },
    });
  }

  // Switches the site to game mode `m` (`pvp`/`pve`/`seasonal`) through its own store action, which also reloads
  // the metadata catalog and syncs the choice to the backend when signed in. ChadScanner's mode wins over the site's.
  async function setMode(m) {
    const store = await until(() => tarkovStore() && tarkovState() && tarkovStore(), 10000, null);
    if (!store) return log('tracker: no progress store for mode ' + m);
    if (tarkovState().mode === m) return;
    if (typeof store.switchGameMode === 'function') await store.switchGameMode(m);
    else store.currentGameMode = m;
    log('tracker: mode → ' + m);
    postProgress();
  }

  // Menu levels ChadScanner read off EFT's screen, written into the site's progress store.
  // `list` is [{id, name, level}] (trader ids); only higher levels are written: a misread or a real loyalty drop
  // must never lower a trader, because lowering makes the site un-complete the stations that depend on it.
  async function setTraders(list) {
    const store = await until(() => tarkovStore() && tarkovState() && tarkovStore(), 10000, null);
    if (!store) return log('tracker: no progress store for trader levels');
    const changes = [];
    for (const t of list) {
      if (store.getTraderLevel(t.id) >= t.level) continue;
      store.setTraderLevel(t.id, t.level);
      changes.push(t.name + ' LL' + t.level);
    }
    if (!changes.length) return;
    toast('Traders', changes.join(', '), false);
    log('tracker: traders ' + changes.join(', '));
    postProgress();
  }

  // Player level ChadScanner read off a trader's screen; raise-only, like trader levels (a misread must not lower it).
  async function setPlayerLevel(level) {
    const store = await until(() => tarkovStore() && tarkovState() && tarkovStore(), 10000, null);
    if (!store) return log('tracker: no progress store for player level');
    const now = typeof store.playerLevel === 'function' ? store.playerLevel() : Number(tarkovState().d.level) || 1;
    if (now >= level) return;
    store.setLevel(level);
    toast('Level', 'Player level ' + now + ' → ' + level, false);
    log('tracker: player level ' + now + ' → ' + level);
  }

  // `list` is [{id, name, level}] (station ids): modules 1..level are completed, never un-completed (EFT never
  // un-builds a station). The site's prerequisite check runs once afterwards; levels it rejects are reported.
  async function setHideout(list) {
    const store = await until(() => tarkovStore() && tarkovState() && tarkovStore(), 10000, null);
    if (!store) return log('tracker: no progress store for hideout levels');
    const written = [];
    for (const s of list) {
      let wrote = false;
      for (let l = 1; l <= s.level; l++) {
        const mid = s.id + '-' + l;
        if (store.isHideoutModuleComplete(mid)) continue;
        store.setHideoutModuleComplete(mid);
        wrote = true;
      }
      if (wrote) written.push(s);
    }
    if (!written.length) return;
    if (typeof store.enforceHideoutPrereqsNow === 'function') store.enforceHideoutPrereqsNow();
    const changes = [];
    const refused = [];
    for (const s of written) {
      (store.isHideoutModuleComplete(s.id + '-' + s.level) ? changes : refused).push(s.name + ' ' + s.level);
    }
    if (changes.length) {
      toast('Hideout', changes.join(', '), false);
      log('tracker: hideout ' + changes.join(', '));
    }
    if (refused.length) {
      toast('Hideout', 'TarkovTracker refused ' + refused.join(', ') + ': unmet trader, skill or station requirement', true);
      log('tracker: hideout refused ' + refused.join(', '));
    }
    postProgress();
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
    mode: (m) => run(() => setMode(m)),
    traders: (l) => run(() => setTraders(l)),
    level: (l) => run(() => setPlayerLevel(l)),
    hideout: (l) => run(() => setHideout(l)),
  };

  // Navigates to the raid's map view (`/tasks/maps?map=<id>`) unless it is already showing.
  async function showRaid() {
    if (!raid) return;
    const r = await until(router, 15000, null);
    if (!r) return log('tracker: page not ready');
    // Before the map list is loaded the site rewrites an unknown `map` to its first map.
    await until(() => document.querySelector('#__nuxt')?.__vue_app__?.config.globalProperties.$pinia?._s?.get('metadata')?.maps?.length, 15000, null);
    const route = r.currentRoute.value;
    let moved = false;
    const m = route.query.map;
    if (route.path !== '/tasks/maps' || (m !== raid.mapId && m !== raid.siteMap)) {
      moved = true;
      await r.replace({ path: '/tasks/maps', query: { map: raid.mapId } });
      if (await until(() => liveMaps().length > 0, 10000, null)) {
        // The site may rewrite a merged id; remembering it avoids re-navigating every time.
        raid.siteMap = r.currentRoute.value.query.map;
      }
    }
    if (!liveMaps().length) return log('tracker: raid map did not load');
    if (moved || !raid.fs) {
      raid.fs = true;
      await openFullscreen();
    }
  }
  const FS_OVERLAY = '[data-testid="map-fullscreen-overlay"]';
  // The site's own full-screen map: fills the window, so no part of the map sits below the page header.
  async function openFullscreen() {
    if (document.querySelector(FS_OVERLAY)) return;
    const b = await until(() => document.querySelector('[data-testid="map-fullscreen-toggle"]'), 5000, null);
    if (!b) return log('tracker: no full-screen button');
    b.click();
    if (!(await until(() => document.querySelector(FS_OVERLAY + ' .leaflet-container'), 5000, null))) log('tracker: full-screen map did not open');
  }
  const exitFullscreen = () => document.querySelector('[data-testid="map-fullscreen-exit"]')?.click();
  const applyRaid = (map) => {
    sweep(map);
    drawHere(map, false);
  };
  Object.assign(window.__chad, {
    raid: (mapId, rotation) =>
      run(async () => {
        const cur = router()?.currentRoute.value.fullPath;
        const back = raid ? raid.back : cur && !cur.startsWith('/tasks/maps') ? cur : '/needed-items';
        raid = { mapId, rotation, keep: null, side: null, here: null, back, fs: false };
        chadOn = true;
        await showRaid();
        liveMaps().forEach(applyRaid);
        refreshAll();
      }),
    raidExtracts: (names, side) =>
      run(() => {
        if (!raid) return;
        raid.keep = names;
        raid.side = side || null;
        refreshAll();
      }),
    here: (p) =>
      run(async () => {
        if (!raid) return;
        raid.here = p;
        await showRaid();
        liveMaps().forEach((m) => drawHere(m, true));
      }),
    raidShow: () => run(showRaid),
    raidEnd: () =>
      run(async () => {
        const back = raid && raid.back;
        raid = null;
        liveMaps().forEach(applyRaid);
        refreshAll();
        const r = router();
        exitFullscreen();
        if (back && r && r.currentRoute.value.path === '/tasks/maps') await r.replace(back);
      }),
  });

  // Chad Fill button next to Smart Fill in the item popup. The popup's counters are component refs, so
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
  // The popup's need rows: task / station level id (from the row's link), FIR, and the count shown ("x/y", at most
  // y, as the Collected totals count it). The app fills only these and leaves the counts of rows it cannot fill.
  const INCREASE_ROW = 'button[aria-label^="Increase objective count for"],button[aria-label^="Increase hideout count for"]';
  const popupRows = (m) =>
    [...m.querySelectorAll('a[href]')].flatMap((a) => {
      const u = new URL(a.getAttribute('href'), location.href);
      if (u.origin !== location.origin) return [];
      const kind = /\/tasks$/.test(u.pathname) ? 'task' : /\/hideout$/.test(u.pathname) ? 'hideout' : null;
      const group = kind && u.searchParams.get(kind === 'task' ? 'task' : 'module');
      if (!group) return [];
      let row = a.parentElement;
      while (row && row !== m && !row.querySelector(INCREASE_ROW)) row = row.parentElement;
      if (!row || row === m || row.querySelectorAll(INCREASE_ROW).length !== 1) return [];
      const shown = /([\d,.\s]+)\/([\d,.\s]+)/.exec(row.querySelector('button[aria-label="Click to enter value"]')?.textContent || '');
      const num = (s) => Number(s.replace(/\D/g, '')) || 0;
      const fir = [...row.querySelectorAll('span')].some((s) => s.textContent.trim() === 'FIR');
      return [{ kind, group, fir, count: shown ? Math.min(num(shown[1]), num(shown[2])) : 0 }];
    });
  // ChadScanner's icon (assets/icon.svg, filled in by the app) for the Fill button.
  const ICON = 'data:image/svg+xml,' + encodeURIComponent(CHAD_ICON_SVG);
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
    const icon = document.createElement('img');
    icon.src = ICON;
    icon.alt = '';
    icon.className = 'mr-1 h-4 w-4 shrink-0';
    b.append(icon, 'Chad Fill');
    b.dataset.chadFill = '1';
    b.title = "Distribute the collected counts in ChadScanner's card order";
    b.onclick = () => {
      const id = modalItemId(m);
      if (!id) return log('tracker: fill: no item id');
      post({ fill: { id, fir: collected(m, 'FIR'), nonFir: collected(m, 'Non-FIR'), rows: popupRows(m) } });
    };
    sf.after(b);
  }
  // The site's map toolbar has PMC / Scav / PMC Spawns toggles; the Chad button next to them is an exclusive mode:
  // on, the three toggles look off and ChadScanner's layers apply; pressing any of them leaves Chad mode.
  function ensureChadLayersButtons() {
    document.querySelectorAll('[data-testid="map-toolbar"]').forEach((toolbar) => {
      if (toolbar.querySelector('[data-chad-layers]')) return;
      const toggles = [...toolbar.querySelectorAll('button[aria-pressed]:not([data-chad-layers])')];
      const pmc = toggles.find((x) => x.textContent.trim() === 'PMC');
      if (!pmc) return;
      const b = document.createElement('button');
      b.type = 'button';
      b.className = baseClass(pmc.className);
      b.dataset.chadLayers = '1';
      b.title =
        "Chad mode: your side's extracts and PMC spawns; in raid only your extracts. Click PMC, Scav or PMC Spawns for the plain TarkovTracker map";
      const icon = document.createElement('img');
      icon.src = ICON;
      icon.alt = '';
      icon.className = 'h-4 w-4 shrink-0';
      const label = document.createElement('span');
      label.className = 'whitespace-nowrap';
      label.textContent = 'Chad';
      b.append(icon, label);
      b.onclick = () => {
        chadOn = !chadOn;
        if (!chadOn) document.querySelectorAll('[data-testid="map-toolbar"]').forEach((t) => setLayers(t, { PMC: false, Scav: false, 'PMC Spawns': false }));
        refreshAll();
      };
      if (!toolbar.dataset.chadListener) {
        toolbar.dataset.chadListener = '1';
        toolbar.addEventListener(
          'click',
          (e) => {
            if (applying || !chadOn) return;
            const t = e.target.closest('button[aria-pressed]');
            if (!t || t.dataset.chadLayers || !LAYERS.includes(t.textContent.trim())) return;
            e.preventDefault();
            e.stopImmediatePropagation();
            chadOn = false;
            const label = t.textContent.trim();
            setLayers(toolbar, { PMC: label === 'PMC', Scav: label === 'Scav', 'PMC Spawns': label === 'PMC Spawns' });
            refreshAll();
          },
          true,
        );
      }
      toggles[toggles.length - 1].after(b);
      ensureChadStyle(toolbar);
      refreshToolbar(toolbar);
    });
  }
  const watch = () =>
    new MutationObserver(() => {
      ensureButton();
      ensureChadLayersButtons();
    }).observe(document.body, { childList: true, subtree: true });
  document.readyState === 'loading' ? document.addEventListener('DOMContentLoaded', watch) : watch();
  // Tells the app this document has __chad: it replays the running raid's calls, which a page (re)load would lose.
  if (window.ipc) window.ipc.postMessage('chad-ready');
})();
