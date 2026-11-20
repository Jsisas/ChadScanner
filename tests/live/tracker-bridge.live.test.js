// Smoke tests of the injected bridge (src/tracker.js) against the live tarkovtracker.org, in a headless Chromium.
//
// No account is involved: every test opens the site in a fresh browser context, which is a signed-out visitor with an
// empty local progress store. The bridge writes only into that throwaway store; nothing reaches the real TarkovTracker
// backend (that needs a login) and nobody's progress is read or changed.
//
// They answer one question a unit test cannot: does the bridge still find what it needs on the site as it is today
// (selectors, router, progress store actions, map toolbar)? A failure here usually means the site changed.
//
//   cd tests/live && npm ci && npx playwright install --with-deps chromium && npm test
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

const ROOT = path.join(__dirname, '..', '..');
const TRACKER_JS = fs.readFileSync(path.join(ROOT, 'src', 'tracker.js'), 'utf8');
const ICON_SVG = fs.readFileSync(path.join(ROOT, 'assets', 'icon.svg'), 'utf8');
// The start page the app opens (src/tracker.rs), and the script exactly as `tracker::script()` builds it.
const START_URL = /START_URL: &str = "([^"]+)"/.exec(fs.readFileSync(path.join(ROOT, 'src', 'tracker.rs'), 'utf8'))[1];
const SCRIPT = TRACKER_JS.replace('CHAD_ICON_SVG', JSON.stringify(ICON_SVG));
const CUSTOMS = '55f2d3fd4bdc2d5f408b4567';

let browser;
test.before(async () => {
  browser = await chromium.launch();
});
test.after(async () => {
  await browser.close();
});

/**
 * A signed-out visitor with the bridge injected the way WebView2 does it: before any page script, with `window.ipc`.
 * `page.app` is what the host app would see; `page.done()` closes the visitor.
 */
async function visit(url = START_URL) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  await context.addInitScript(() => {
    window.__ipc = [];
    window.ipc = { postMessage: (m) => window.__ipc.push(m) };
  });
  await context.addInitScript(SCRIPT);
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__chad, null, { timeout: 30000 });
  page.errors = errors;
  page.done = () => context.close();
  page.app = {
    /** Everything the page posted to the app since the last `clear`, parsed. */
    messages: async () => (await page.evaluate(() => window.__ipc.slice())).map((m) => (m === 'chad-ready' ? m : JSON.parse(m))),
    clear: () => page.evaluate(() => (window.__ipc.length = 0)),
    /** Waits for a message `{[key]: payload}` whose payload satisfies `pred`; returns the payload. */
    until: async (key, pred = () => true, timeout = 30000) => {
      const end = Date.now() + timeout;
      for (;;) {
        const msgs = await page.app.messages();
        const hit = msgs.map((m) => m && m[key]).filter((v) => v !== undefined).find(pred);
        if (hit !== undefined) return hit;
        if (Date.now() > end) throw new Error(`no "${key}" message matched within ${timeout} ms; the app saw ${JSON.stringify(msgs)}`);
        await page.waitForTimeout(100);
      }
    },
  };
  return page;
}

/** Runs `fn(pinia, ...args)` in the page: the site's stores, as the bridge reaches them. */
const site = (page, fn, ...args) =>
  page.evaluate(
    ([src, a]) => eval('(' + src + ')')(document.querySelector('#__nuxt').__vue_app__.config.globalProperties.$pinia, ...a),
    [fn.toString(), args],
  );

const neededItemsLoaded = (page) => page.waitForSelector('[data-testid="needed-item-group"]', { timeout: 60000 });

/** A needed item (card id, name) that has a hideout need, with the hideout part that wants it and its station level. */
async function hideoutItem(page) {
  await neededItemsLoaded(page);
  return site(page, (pinia) => {
    const parts = new Map();
    for (const st of pinia._s.get('metadata').hideoutStations)
      for (const lvl of st.levels) for (const p of lvl.itemRequirements) if (!parts.has(p.item.id)) parts.set(p.item.id, { part: p, level: lvl.id });
    for (const g of document.querySelectorAll('[data-testid="needed-item-group"]')) {
      const id = /\/([0-9a-f]{24})-/.exec(g.querySelector('img')?.getAttribute('src') || '')?.[1];
      const name = [...g.querySelectorAll('h3')].map((h) => h.textContent.trim())[0];
      if (id && name && parts.has(id)) {
        const { part, level } = parts.get(id);
        return { id, name, partId: part.id, required: part.count, level };
      }
    }
    return null;
  });
}

test('the bridge defines its API and announces itself on the site, and nowhere else', async () => {
  const page = await visit();
  const api = await page.evaluate(() => Object.keys(window.__chad).sort());
  assert.deepEqual(api, ['fill', 'hideout', 'here', 'level', 'mode', 'open', 'raid', 'raidEnd', 'raidExtracts', 'raidShow', 'step', 'traders'].sort());
  assert.ok((await page.app.messages()).includes('chad-ready'), 'chad-ready is what makes the app replay a raid in progress');
  assert.deepEqual(page.errors, [], 'the injected script must not break the page');
  await page.goto('about:blank');
  assert.equal(await page.evaluate(() => window.__chad), undefined, 'it must stay inert on other hosts');
  await page.done();
});

test('the site still exposes the router and the progress store actions the bridge calls', async () => {
  const page = await visit();
  await neededItemsLoaded(page);
  // Every `store.<fn>(` the script calls on the site's progress store.
  const called = [...new Set([...TRACKER_JS.matchAll(/\bstore\.([A-Za-z]+)\(/g)].map((m) => m[1]))];
  assert.ok(called.length >= 12, `expected the script to call many store actions, found ${called}`);
  const missing = await site(page, (pinia, names) => names.filter((n) => typeof pinia._s.get('swapTarkov')[n] !== 'function'), called);
  assert.deepEqual(missing, [], 'the site store lost actions tracker.js calls');
  const shape = await site(page, (pinia) => {
    const st = pinia.state.value.swapTarkov;
    return {
      router: typeof document.querySelector('#__nuxt').__vue_app__.config.globalProperties.$router?.replace,
      mode: st.currentGameMode,
      section: typeof st[st.currentGameMode],
      hiddenTaskIds: Array.isArray(pinia.state.value.preferences?.hiddenTaskIds),
    };
  });
  assert.equal(shape.router, 'function');
  assert.ok(['pvp', 'pve', 'seasonal'].includes(shape.mode), shape.mode);
  assert.equal(shape.section, 'object', 'progress lives under swapTarkov[currentGameMode]');
  assert.ok(shape.hiddenTaskIds, 'preferences.hiddenTaskIds is how hidden quests are read');
  assert.deepEqual(await page.app.until('hidden'), { tasks: [] }, 'a signed-out visitor hides nothing');
  await page.done();
});

test('the needed-items page has the cards and the item-once switch the bridge looks for', async () => {
  const page = await visit();
  await neededItemsLoaded(page);
  // Images load lazily: wait until some cards show their item id.
  const cardsOf = () =>
    page.evaluate(() =>
      [...document.querySelectorAll('[data-testid="needed-item-group"]')].map((g) => ({
        id: /\/([0-9a-f]{24})-/.exec(g.querySelector('img')?.getAttribute('src') || '')?.[1],
        name: [...g.querySelectorAll('h3')].map((h) => h.textContent.trim())[0],
      })),
    );
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="needed-item-group"] img[src*="assets.tarkov.dev/"]').length > 5, null, { timeout: 30000 });
  const cards = await cardsOf();
  assert.ok(cards.length > 5, `only ${cards.length} item cards`);
  assert.ok(cards.every((c) => c.name), 'each card names its item in an h3');
  assert.ok(cards.filter((c) => c.id).length > 5, 'item cards carry the item id in their image URL');
  assert.equal(await page.locator('[data-testid="needed-items-show"] [role=switch]').count(), 1, '"Show each item once" switch');
  await page.done();
});

test('open shows the item popup with a Chad Fill button, and clicking it reports the popup rows', async () => {
  const page = await visit();
  const item = await hideoutItem(page);
  assert.ok(item, 'no needed item with a hideout need');
  await page.app.clear();
  await page.evaluate(([id, name]) => window.__chad.open(id, name), [item.id, item.name]);
  await page.locator('[role=dialog][data-state=open] [data-chad-fill]').waitFor({ timeout: 30000 });
  assert.equal(await page.app.until('log', (l) => l === `tracker: ${item.name} opened`), `tracker: ${item.name} opened`);
  await page.locator('[data-chad-fill]').click();
  const msg = await page.app.until('fill');
  assert.equal(msg.id, item.id);
  assert.equal(typeof msg.fir, 'number');
  assert.equal(typeof msg.nonFir, 'number');
  assert.ok(msg.rows.length >= 1, 'the popup lists at least the need that put the item on the page');
  for (const r of msg.rows) {
    assert.ok(['task', 'hideout'].includes(r.kind), r.kind);
    assert.ok(typeof r.group === 'string' && r.group.length > 0);
    assert.equal(typeof r.fir, 'boolean');
    assert.ok(Number.isInteger(r.count) && r.count >= 0);
  }
  assert.ok(
    msg.rows.some((r) => r.kind === 'hideout' && r.group === item.level),
    'the hideout level row is named by the station level id',
  );
  await page.done();
});

test('open reports an item that is not among the needed items', async () => {
  const page = await visit();
  await neededItemsLoaded(page);
  await page.app.clear();
  await page.evaluate(() => window.__chad.open('000000000000000000000000', 'Definitely not an item'));
  await page.app.until('log', (l) => /not in needed items/.test(l), 60000);
  assert.equal(await page.locator('[role=dialog][data-state=open]').count(), 0);
  await page.done();
});

test('Arrow Up/Down and Chad Fill write hideout part counts through the site store', async () => {
  const page = await visit();
  const item = await hideoutItem(page);
  assert.ok(item);
  const target = { kind: 'hideout', id: item.partId, find: null, required: item.required, fir: false, label: 'Test 1' };
  const count = () => site(page, (pinia, id) => pinia._s.get('swapTarkov').getHideoutPartCount(id), item.partId);

  await page.app.clear();
  await page.evaluate(([i, t]) => window.__chad.step(i.id, i.name, true, [t]), [item, target]);
  const up = await page.app.until('progress', (p) => p.parts[item.partId]?.count === 1);
  assert.deepEqual(up.parts[item.partId], { count: 1, complete: item.required <= 1 });
  assert.ok(['pvp', 'pve', 'seasonal'].includes(up.mode));
  assert.equal(await count(), 1);
  assert.equal((await page.app.until('toast')).warn, false);

  await page.app.clear();
  await page.evaluate(([i, t]) => window.__chad.step(i.id, i.name, false, [t]), [item, target]);
  await page.app.until('progress', (p) => p.parts[item.partId]?.count === 0);
  assert.equal(await count(), 0);

  // Chad Fill works on the item's open popup.
  await page.evaluate(([id, name]) => window.__chad.open(id, name), [item.id, item.name]);
  await page.locator('[role=dialog][data-state=open] [data-chad-fill]').waitFor({ timeout: 30000 });
  const n = Math.min(2, item.required);
  await page.app.clear();
  await page.evaluate(([i, t, n]) => window.__chad.fill(i.id, i.name, [t], [n], 0, 0), [item, target, n]);
  await page.app.until('progress', (p) => p.parts[item.partId]?.count === n);
  assert.equal(await count(), n);
  await page.done();
});

test('player, trader and hideout levels are raised in the site store and never lowered', async () => {
  const page = await visit();
  await neededItemsLoaded(page);
  const level = () => site(page, (pinia) => pinia._s.get('swapTarkov').playerLevel());
  await page.evaluate(() => window.__chad.level(26));
  await page.app.until('toast', (t) => t.title === 'Level');
  assert.equal(await level(), 26);
  await page.app.clear();
  await page.evaluate(() => window.__chad.level(10));
  await page.waitForTimeout(1500);
  assert.equal(await level(), 26, 'a lower level is a misread and is ignored');
  assert.deepEqual((await page.app.messages()).filter((m) => m.toast || m.progress), [], 'and nothing is announced');

  const trader = () => site(page, (pinia) => pinia._s.get('swapTarkov').getTraderLevel('chad-test-trader'));
  await page.evaluate(() => window.__chad.traders([{ id: 'chad-test-trader', name: 'Tester', level: 3 }]));
  await page.app.until('toast', (t) => t.title === 'Traders' && /Tester LL3/.test(t.body));
  assert.equal(await trader(), 3);
  await page.evaluate(() => window.__chad.traders([{ id: 'chad-test-trader', name: 'Tester', level: 2 }]));
  await page.waitForTimeout(1500);
  assert.equal(await trader(), 3, 'trader levels only go up');

  // Level 1 of a station with nothing in front of it.
  const station = await site(page, (pinia) => {
    const s = pinia._s.get('metadata').hideoutStations.find((x) => x.levels[0] && !x.levels[0].stationLevelRequirements.length && !x.levels[0].traderRequirements.length);
    return s && { id: s.id, name: s.name };
  });
  assert.ok(station, 'no station level 1 without prerequisites');
  await page.app.clear();
  await page.evaluate((s) => window.__chad.hideout([{ id: s.id, name: s.name, level: 1 }]), station);
  await page.app.until('toast', (t) => t.title === 'Hideout');
  assert.equal(await site(page, (pinia, id) => pinia._s.get('swapTarkov').isHideoutModuleComplete(id + '-1'), station.id), true);
  await page.done();
});

test('mode switches the site to the game mode ChadScanner is in', async () => {
  const page = await visit();
  await neededItemsLoaded(page);
  for (const mode of ['pve', 'seasonal', 'pvp']) {
    await page.app.clear();
    await page.evaluate((m) => window.__chad.mode(m), mode);
    await page.app.until('progress', (p) => p.mode === mode, 60000);
    assert.equal(await site(page, (pinia) => pinia._s.get('swapTarkov').currentGameMode), mode);
  }
  await page.done();
});

test('the raid map opens full screen with the Chad layers button, keeps only the raid extracts and draws the player', async () => {
  const page = await visit();
  await neededItemsLoaded(page);
  await page.evaluate((id) => window.__chad.raid(id, 180), CUSTOMS);
  await page.waitForURL(new RegExp(`/tasks/maps\\?map=${CUSTOMS}`), { timeout: 60000 });
  await page.locator('[data-testid="map-fullscreen-overlay"] .leaflet-container').waitFor({ timeout: 60000 });
  await page.locator('[data-chad-layers]').first().waitFor({ timeout: 30000 });

  // The layer panel rows the bridge clicks. `shared` and `coop` only exist on some maps/modes (seen: Customs with and without `coop`), so only the always-present ones are required.
  const rows = await page.evaluate(async () => {
    const toggle = document.querySelector('[data-testid="map-fullscreen-overlay"] [data-testid="map-layers-toggle"]') || document.querySelector('[data-testid="map-layers-toggle"]');
    if (toggle.getAttribute('aria-expanded') !== 'true') toggle.click();
    for (let i = 0; i < 50 && !document.getElementById(toggle.getAttribute('aria-controls')); i++) await new Promise((r) => setTimeout(r, 100));
    const panel = document.getElementById(toggle.getAttribute('aria-controls'));
    return panel ? [...panel.querySelectorAll('button[data-layer]')].map((b) => b.dataset.layer) : [];
  });
  for (const layer of ['pmc', 'scav', 'spawns']) assert.ok(rows.includes(layer), `layer row ${layer} in ${rows}`);

  // Extract badges: the names the bridge matches, and hides the rest by.
  const badges = () =>
    page.evaluate(() =>
      [...document.querySelectorAll('.extract-marker:not(.transit-marker)')].map((e) => ({
        name: (e.querySelector('[aria-label]') || e).getAttribute('aria-label') || e.textContent.trim(),
        shown: (e.closest('.leaflet-marker-icon') || e).style.display !== 'none',
      })),
    );
  await page.waitForFunction(() => document.querySelectorAll('.extract-marker').length > 0, null, { timeout: 30000 });
  const all = await badges();
  assert.ok(all.length >= 2 && all.every((b) => b.name), `extract badges: ${JSON.stringify(all)}`);
  const keep = all[0].name.split(',')[0];
  await page.evaluate((n) => window.__chad.raidExtracts([n], 'pmc'), keep);
  await page.waitForFunction(() => [...document.querySelectorAll('.extract-marker:not(.transit-marker)')].some((e) => (e.closest('.leaflet-marker-icon') || e).style.display === 'none'), null, {
    timeout: 20000,
  });
  const filtered = await badges();
  assert.ok(filtered.filter((b) => b.name.split(',')[0] === keep).every((b) => b.shown), 'the raid extract stays');
  assert.ok(filtered.some((b) => b.name.split(',')[0] !== keep && !b.shown), 'other extracts are hidden');

  // The site appends qualifiers after a comma ("RUAF Roadblock, Unknown"); the kept names (json.tarkov.dev) never carry them.
  const qualified = all.find((b) => b.name.includes(','));
  assert.ok(qualified, `an extract badge with a ", qualifier" label on Customs: ${JSON.stringify(all.map((b) => b.name))}`);
  const bare = qualified.name.split(',')[0];
  await page.evaluate((n) => window.__chad.raidExtracts([n], 'pmc'), bare);
  await page.waitForFunction(
    (q) => [...document.querySelectorAll('.extract-marker:not(.transit-marker)')].some((e) => (e.querySelector('[aria-label]') || e).getAttribute('aria-label') === q && (e.closest('.leaflet-marker-icon') || e).style.display !== 'none') &&
      [...document.querySelectorAll('.extract-marker:not(.transit-marker)')].some((e) => (e.closest('.leaflet-marker-icon') || e).style.display === 'none'),
    qualified.name,
    { timeout: 20000 },
  );
  const requalified = await badges();
  assert.ok(requalified.filter((b) => b.name === qualified.name).every((b) => b.shown), 'the qualified raid extract stays');
  assert.ok(requalified.some((b) => b.name.split(',')[0] !== bare && !b.shown), 'other extracts are hidden');

  // The player's arrow.
  await page.evaluate(() => window.__chad.here({ pos: [0, 0], yaw: 90 }));
  await page.locator('.chad-me').first().waitFor({ state: 'attached', timeout: 20000 });

  // End of raid: arrow gone, full screen left.
  await page.evaluate(() => window.__chad.raidEnd());
  await page.waitForFunction(() => !document.querySelector('.chad-me'), null, { timeout: 20000 });
  await page.waitForSelector('[data-testid="map-fullscreen-overlay"]', { state: 'detached', timeout: 20000 });
  assert.deepEqual(page.errors, []);
  await page.done();
});
