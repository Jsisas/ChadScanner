var RELEASE_API = 'https://api.github.com/repos/Jsisas/ChadScanner/releases/latest';
var ASSET_RE = /^chadscanner-.+-win-x64\.zip$/;

function applyRelease(rel) {
  if (!rel || !rel.tag_name || !Array.isArray(rel.assets)) return false;
  var asset = rel.assets.find(function (a) { return ASSET_RE.test(a.name); });
  if (!asset) return false;
  var mb = (asset.size / 1048576).toFixed(1);
  ['download-btn', 'download-btn-2'].forEach(function (id) {
    var el = document.getElementById(id);
    if (!el) return;
    el.href = asset.browser_download_url;
    el.textContent = 'Download ' + rel.tag_name;
  });
  ['download-meta', 'download-meta-2'].forEach(function (id) {
    var el = document.getElementById(id);
    if (el) el.textContent = rel.tag_name + ' \u00b7 ' + mb + ' MB zip \u00b7 Windows 10/11 x64';
  });
  return true;
}

document.addEventListener('DOMContentLoaded', function () {
  fetch(RELEASE_API, { headers: { Accept: 'application/vnd.github+json' } })
    .then(function (res) { return res.ok ? res.json() : null; })
    .then(function (rel) { if (rel) applyRelease(rel); })
    .catch(function () {});
});
