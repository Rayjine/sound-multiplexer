// Two jobs: (1) point the download cards at the latest release's assets;
// (2) drive the interactive app replica in the hero. Nothing here talks to
// a backend — the replica is a toy that behaves like the real app.
(function () {
  'use strict';

  /* ============ download cards ============ */
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

  /* ============ app replica ============ */
  // Same icons and semantics as the real app: mute toggles red, switches
  // flip, sliders drag, scrolling over a row nudges its volume, the master
  // row only exists while two or more devices are enabled, and the status
  // line mirrors the app's wording.
  var ICON_UNMUTED = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5H4z" fill="currentColor" stroke-linejoin="round"/><path d="M15.5 9a4.2 4.2 0 0 1 0 6M18 6.5a8 8 0 0 1 0 11"/></svg>';
  var ICON_MUTED = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5H4z" fill="currentColor" stroke-linejoin="round"/><path d="M15.5 9.5l5 5M20.5 9.5l-5 5"/></svg>';
  var WHEEL_STEP = 5;

  var win = document.querySelector('.appwin');
  if (!win) return;
  var rowEls = Array.prototype.slice.call(win.querySelectorAll('.aw-device'));
  var statusText = win.querySelector('.aw-status-text');
  var statusDot = win.querySelector('.aw-dot');

  function clamp(v) { return Math.max(0, Math.min(100, v)); }

  rowEls.forEach(function (li) {
    var isMaster = li.classList.contains('aw-master');
    var name = li.dataset.name;
    var vol = li.querySelector('.aw-vol');
    vol.innerHTML =
      '<button class="aw-mute" aria-pressed="false" aria-label="Mute ' + name + '">' +
        '<span class="aw-ic-un">' + ICON_UNMUTED + '</span>' +
        '<span class="aw-ic-mu">' + ICON_MUTED + '</span>' +
      '</button>' +
      '<span class="aw-track"><span class="aw-fill"></span><span class="aw-thumb"></span></span>' +
      '<span class="aw-pct"></span>';

    var track = vol.querySelector('.aw-track');
    var fill = vol.querySelector('.aw-fill');
    var thumb = vol.querySelector('.aw-thumb');
    var pctEl = vol.querySelector('.aw-pct');
    var mute = vol.querySelector('.aw-mute');
    var sw = li.querySelector('.aw-switch');

    track.setAttribute('tabindex', '0');
    track.setAttribute('role', 'slider');
    track.setAttribute('aria-label', 'Volume for ' + name);
    track.setAttribute('aria-valuemin', '0');
    track.setAttribute('aria-valuemax', '100');

    function paint() {
      var pct = Number(li.dataset.vol);
      fill.style.width = pct + '%';
      thumb.style.left = pct + '%';
      pctEl.textContent = pct + '%';
      track.setAttribute('aria-valuenow', String(pct));
    }
    paint();

    function nudge(step) {
      li.dataset.vol = clamp(Number(li.dataset.vol) + step);
      paint();
    }

    mute.addEventListener('click', function () {
      var m = li.dataset.muted !== 'true';
      li.dataset.muted = String(m);
      mute.setAttribute('aria-pressed', String(m));
      mute.setAttribute('aria-label', (m ? 'Unmute ' : 'Mute ') + name);
    });

    if (sw) {
      sw.addEventListener('click', function () {
        var on = li.dataset.on !== 'true';
        li.dataset.on = String(on);
        sw.setAttribute('aria-pressed', String(on));
        updateStatus();
      });
    }

    function setFromPointer(e) {
      var r = track.getBoundingClientRect();
      li.dataset.vol = clamp(Math.round(((e.clientX - r.left) / r.width) * 100));
      paint();
    }
    track.addEventListener('pointerdown', function (e) {
      e.preventDefault();
      track.setPointerCapture(e.pointerId);
      setFromPointer(e);
    });
    track.addEventListener('pointermove', function (e) {
      if (track.hasPointerCapture && track.hasPointerCapture(e.pointerId)) setFromPointer(e);
    });
    track.addEventListener('keydown', function (e) {
      var step = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1 }[e.key];
      if (step) { e.preventDefault(); nudge(step * WHEEL_STEP); }
      else if (e.key === 'Home') { e.preventDefault(); li.dataset.vol = 0; paint(); }
      else if (e.key === 'End') { e.preventDefault(); li.dataset.vol = 100; paint(); }
    });

    // Wheel only over the volume cluster (not the whole row), so scrolling
    // the page across the replica keeps working. Discrete notches step once;
    // fine touchpad deltas accumulate — same normalization as the app.
    var wheelAcc = 0;
    vol.addEventListener('wheel', function (e) {
      if (!e.deltaY) return;
      e.preventDefault();
      var px = e.deltaMode === 1 ? e.deltaY * 40 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
      var step = 0;
      if (Math.abs(px) >= 30) { wheelAcc = 0; step = px < 0 ? 1 : -1; }
      else {
        wheelAcc += px;
        if (Math.abs(wheelAcc) >= 30) { step = wheelAcc < 0 ? 1 : -1; wheelAcc = 0; }
      }
      if (step) nudge(step * WHEEL_STEP);
    }, { passive: false });
  });

  function updateStatus() {
    var real = rowEls.filter(function (li) { return !li.classList.contains('aw-master'); });
    var n = real.filter(function (li) { return li.dataset.on === 'true'; }).length;
    var master = win.querySelector('.aw-master');
    // The real app only shows the master row while a combine sink exists,
    // i.e. with two or more devices enabled.
    master.style.display = n >= 2 ? '' : 'none';
    if (n === 0) {
      statusText.textContent = 'Silent mode — no devices selected';
      statusDot.dataset.state = 'silent';
    } else {
      statusText.textContent = n + (n === 1 ? ' device active' : ' devices active');
      statusDot.dataset.state = 'active';
    }
  }
  updateStatus();
})();
