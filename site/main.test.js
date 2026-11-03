const test = require('node:test');
const assert = require('node:assert');

const els = {};
['download-btn', 'download-btn-2', 'download-meta', 'download-meta-2'].forEach(function (id) {
  els[id] = {};
});
global.document = {
  getElementById: function (id) { return els[id] || null; },
  addEventListener: function () {},
};
const { applyRelease } = require('./main.js');

function reset() {
  Object.keys(els).forEach(function (k) { els[k] = {}; });
}

test('picks_app_zip_over_data_zip', function () {
  reset();
  const ok = applyRelease({
    tag_name: 'v0.1.0',
    assets: [
      { name: 'chadscanner-data-ab12.zip', size: 1, browser_download_url: 'https://x/data' },
      { name: 'chadscanner-0.1.0-win-x64.zip', size: 9437184, browser_download_url: 'https://x/app' },
    ],
  });
  assert.strictEqual(ok, true);
  assert.strictEqual(els['download-btn'].href, 'https://x/app');
});

test('sets_both_buttons_and_meta', function () {
  reset();
  applyRelease({
    tag_name: 'v0.1.0',
    assets: [{ name: 'chadscanner-0.1.0-win-x64.zip', size: 9437184, browser_download_url: 'u' }],
  });
  ['download-btn', 'download-btn-2'].forEach(function (id) {
    assert.strictEqual(els[id].textContent, 'Download v0.1.0');
  });
  ['download-meta', 'download-meta-2'].forEach(function (id) {
    assert.strictEqual(els[id].textContent, 'v0.1.0 \u00b7 9.0 MB zip \u00b7 Windows 10/11 x64');
  });
});

test('rejects_incomplete_release', function () {
  [null, { assets: [] }, { tag_name: 'v1' }, { tag_name: 'v1', assets: [] }].forEach(function (rel) {
    reset();
    assert.strictEqual(applyRelease(rel), false);
    assert.deepStrictEqual(els['download-btn'], {});
  });
});

test('match_is_case_sensitive', function () {
  reset();
  const ok = applyRelease({
    tag_name: 'v1',
    assets: [{ name: 'ChadScanner-0.1.0-win-x64.zip', size: 1, browser_download_url: 'u' }],
  });
  assert.strictEqual(ok, false);
});
