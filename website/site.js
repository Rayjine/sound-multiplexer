// Point the download cards at the latest release's assets. The static
// hrefs (the releases page) remain as fallback when the API call fails
// or no release has been published yet.
(function () {
  var matchers = {
    appimage: function (n) { return n.endsWith(".AppImage"); },
    deb: function (n) { return n.endsWith(".deb"); },
    rpm: function (n) { return n.endsWith(".rpm"); },
    windows: function (n) { return n.endsWith(".exe"); },
  };

  fetch("https://api.github.com/repos/Rayjine/sound-multiplexer/releases/latest")
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (rel) {
      if (!rel || !rel.assets) return;
      Object.keys(matchers).forEach(function (kind) {
        var asset = rel.assets.find(function (a) { return matchers[kind](a.name); });
        var card = document.querySelector('[data-asset="' + kind + '"]');
        if (asset && card) card.href = asset.browser_download_url;
      });
      document.querySelectorAll("[data-version]").forEach(function (el) {
        el.textContent = rel.tag_name;
        el.hidden = false;
      });
    })
    .catch(function () { /* fallback hrefs stay */ });
})();
