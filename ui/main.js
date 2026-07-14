/* Sound Multiplexer UI. Framework-free, no build step: index.html + this
   file + styles.css are loaded as-is by the Tauri webview.

   State flow — one source of truth:
   - The backend's full device list is the only authoritative state; it
     arrives via the `devices-changed` event and the get/refresh commands.
     Mutation commands (set_*) return nothing — their effect comes back as
     the next `devices-changed`.
   - Handlers apply mutations optimistically for instant feedback, then
     invoke the command; on rejection revertOnError() refetches, so the UI
     never keeps showing routing the backend refused.

   Rendering:
   - Keyed rows (`rows`: id -> <li>) updated in place; reordering moves only
     misplaced nodes. Re-appending an already-placed node would blur focus
     and kill an in-progress slider drag, and the backend emits
     devices-changed bursts during routing changes.
   - `draggingId` shields the slider being dragged from incoming volume
     writes, which lag behind the local drag position.

   Master row: while 2+ devices are enabled the backend prepends a synthetic
   `deviceType: 'master'` device (the combine sink itself). It renders like
   any row, but realDevices() excludes it from the status count and the
   empty check, and CSS hides its enable switch ([data-master="true"]).

   Volume and mute stay adjustable on disabled rows: they set the device's
   stored level/mute without touching routing.

   Without window.__TAURI__ (plain browser, jsdom) an in-memory mock backend
   activates, so the UI can be previewed standalone and DOM-tested
   (ui-tests/ drives both the mock and a fake __TAURI__). */
(function () {
  'use strict';

  /* ================= IPC layer ================= */
  var TAURI = window.__TAURI__ || null;

  var mockDevices = [
    { id: 'mock.analog', name: 'Built-in Audio Analog Stereo', deviceType: 'speakers', enabled: true, volume: 0.65, muted: false },
    { id: 'mock.hp', name: 'HD 560S Analog Stereo', deviceType: 'headphones', enabled: true, volume: 0.4, muted: false },
    { id: 'mock.bt', name: 'WH-1000XM5', deviceType: 'bluetooth', enabled: false, volume: 0.8, muted: false },
    { id: 'mock.hdmi', name: 'Navi 21/23 HDMI Audio [Radeon RX 6800]', deviceType: 'hdmi', enabled: false, volume: 1.0, muted: true },
  ];

  var api = TAURI
    ? {
        getDevices: function () { return TAURI.core.invoke('get_devices'); },
        refreshDevices: function () { return TAURI.core.invoke('refresh_devices'); },
        setEnabled: function (id, enabled) { return TAURI.core.invoke('set_device_enabled', { id: id, enabled: enabled }); },
        setAllEnabled: function (enabled) { return TAURI.core.invoke('set_all_enabled', { enabled: enabled }); },
        setVolume: function (id, volume) { return TAURI.core.invoke('set_device_volume', { id: id, volume: volume }); },
        setMuted: function (id, muted) { return TAURI.core.invoke('set_device_muted', { id: id, muted: muted }); },
        onDevicesChanged: function (cb) { TAURI.event.listen('devices-changed', function (e) { cb(e.payload); }); },
        onBackendError: function (cb) { TAURI.event.listen('backend-error', function (e) { cb(e.payload); }); },
      }
    : {
        _listener: null,
        _emit: function () {
          var self = this;
          if (this._listener) {
            setTimeout(function () {
              self.getDevices().then(self._listener);
            }, 0);
          }
        },
        getDevices: function () { return Promise.resolve(mockDevices.map(function (d) { return Object.assign({}, d); })); },
        refreshDevices: function () { return this.getDevices(); },
        setEnabled: function (id, enabled) {
          mockDevices.forEach(function (d) { if (d.id === id) d.enabled = enabled; });
          this._emit();
          return Promise.resolve();
        },
        setAllEnabled: function (enabled) {
          mockDevices.forEach(function (d) { d.enabled = enabled; });
          this._emit();
          return Promise.resolve();
        },
        setVolume: function (id, volume) {
          mockDevices.forEach(function (d) { if (d.id === id) d.volume = volume; });
          // No emit: an echoed devices-changed would fight the active drag.
          return Promise.resolve();
        },
        setMuted: function (id, muted) {
          mockDevices.forEach(function (d) { if (d.id === id) d.muted = muted; });
          this._emit();
          return Promise.resolve();
        },
        onDevicesChanged: function (cb) { this._listener = cb; },
        onBackendError: function () {},
      };

  /* ================= device-type icons ================= */
  var ICONS = {
    speakers: '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="7" y="3" width="10" height="18" rx="2"/><circle cx="12" cy="8" r="1.4" fill="currentColor" stroke="none"/><circle cx="12" cy="15" r="3"/></svg>',
    headphones: '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M4 14v-1a8 8 0 0 1 16 0v1"/><rect x="3" y="14" width="4" height="6" rx="2"/><rect x="17" y="14" width="4" height="6" rx="2"/></svg>',
    bluetooth: '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M6.5 7.5l11 9L12 21V3l5.5 4.5-11 9"/></svg>',
    hdmi: '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="4.5" width="18" height="12.5" rx="2"/><path d="M12 17v3.5M8 20.5h8"/></svg>',
    usb: '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M12 5v13"/><path d="M12 5l-2 2.6M12 5l2 2.6" stroke-linecap="round"/><path d="M12 12.5L7.5 10M12 15l4.5-2.5"/><circle cx="7" cy="9" r="1.6"/><rect x="15" y="10" width="3.2" height="3.2"/><circle cx="12" cy="19.5" r="1.8" fill="currentColor" stroke="none"/></svg>',
    digital: '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="2.6" fill="currentColor" stroke="none"/><path d="M12 4v2.2"/></svg>',
    // The app's own brand mark: the master row is the combined output itself.
    master: '<svg width="20" height="20" viewBox="0 0 16 16" fill="currentColor"><rect x="2.5" y="6" width="2" height="8" rx="1"/><rect x="7" y="2" width="2" height="12" rx="1"/><rect x="11.5" y="4" width="2" height="10" rx="1"/></svg>',
  };
  var TYPE_LABELS = {
    speakers: 'Speakers',
    headphones: 'Headphones',
    bluetooth: 'Bluetooth',
    hdmi: 'HDMI / display audio',
    usb: 'USB audio',
    digital: 'Digital / optical',
    master: 'Combined output — controls all enabled devices at once',
  };

  /* ================= state & rendering ================= */
  var app = document.getElementById('app');
  var list = document.getElementById('deviceList');
  var statusbar = document.getElementById('statusbar');
  var statusText = document.getElementById('statusText');
  var template = document.getElementById('deviceRowTemplate');

  var devices = [];          // last known full state
  var rows = new Map();      // device id -> <li> element
  var draggingId = null;     // device whose slider is being dragged
  var volumeTouched = new Map(); // device id -> ts of last local volume write
  var WHEEL_STEP = 5;        // % per wheel notch when scrolling on a row
  /* Snapshots listing a device's volume can be read mid-drag but delivered
     after the drag ended (draggingId already cleared); nothing corrects the
     stale value afterwards, because the backend suppresses the echo of our
     own final write. Ignore snapshot volumes for a device this long after
     the last local write — long enough for an in-flight snapshot (80ms pump
     coalescing + list + emit), short enough for real external changes. */
  var VOLUME_ECHO_GRACE_MS = 600;

  function updateStatus() {
    app.dataset.empty = String(realDevices().length === 0);
    // An active error flash owns the status bar until its timer restores it;
    // without this, the revert-path refetch would repaint the derived status
    // over the error before the user could read it.
    if (errorFlashTimer) return;
    if (realDevices().length === 0) {
      statusbar.dataset.state = 'error';
      statusText.textContent = 'No output devices found';
      return;
    }
    var n = realDevices().filter(function (d) { return d.enabled; }).length;
    if (n === 0) {
      statusbar.dataset.state = 'silent';
      statusText.textContent = 'Silent mode — no devices selected';
    } else {
      statusbar.dataset.state = 'active';
      statusText.textContent = n + (n === 1 ? ' device active' : ' devices active');
    }
  }

  function volumePct(d) {
    return Math.round(d.volume * 100);
  }

  /* The master row is the routing itself, not a selectable device. */
  function realDevices() {
    return devices.filter(function (d) { return d.deviceType !== 'master'; });
  }

  function createRow(d) {
    var li = template.content.firstElementChild.cloneNode(true);
    li.dataset.id = d.id;
    li.querySelector('.dev-icon').innerHTML = ICONS[d.deviceType] || ICONS.speakers;

    var mute = li.querySelector('.mute-btn');
    var slider = li.querySelector('input[type="range"]');
    var sw = li.querySelector('.switch');

    sw.addEventListener('click', function () {
      var on = li.dataset.enabled !== 'true';
      applyDeviceUpdate(Object.assign({}, findDevice(d.id), { enabled: on }));
      api.setEnabled(d.id, on).catch(revertOnError);
    });

    mute.addEventListener('click', function () {
      var m = li.dataset.muted !== 'true';
      applyDeviceUpdate(Object.assign({}, findDevice(d.id), { muted: m }));
      api.setMuted(d.id, m).catch(revertOnError);
    });

    // input = live drag: update locally, push over IPC at most every 80ms.
    // change = drag end: clear the guard and push the final value unthrottled.
    var pushVolume = throttle(function (value) {
      volumeTouched.set(d.id, Date.now());
      api.setVolume(d.id, value / 100).catch(surfaceError);
    }, 80);
    var wheelEndTimer = null;
    slider.addEventListener('input', function () {
      // The slider takes over any wheel session; a stale wheel-end push
      // would otherwise override the newer drag value.
      clearTimeout(wheelEndTimer);
      wheelEndTimer = null;
      draggingId = d.id;
      var dev = findDevice(d.id);
      if (dev) dev.volume = slider.value / 100;
      slider.style.setProperty('--val', slider.value);
      li.querySelector('.pct').value = slider.value + '%';
      pushVolume(Number(slider.value));
    });
    slider.addEventListener('change', function () {
      clearTimeout(wheelEndTimer);
      wheelEndTimer = null;
      draggingId = null;
      volumeTouched.set(d.id, Date.now());
      api.setVolume(d.id, Number(slider.value) / 100).catch(surfaceError);
    });

    // Scrolling on the row nudges the volume (pavucontrol-style). A discrete
    // notch (>= 30px equivalent) is one step no matter its reported size;
    // finer touchpad deltas accumulate, so a flick can't slam the volume.
    var wheelAcc = 0;
    function wheelStep(e) {
      var px = e.deltaMode === 1 ? e.deltaY * 40 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
      if (Math.abs(px) >= 30) { wheelAcc = 0; return px < 0 ? 1 : -1; }
      wheelAcc += px;
      if (Math.abs(wheelAcc) < 30) return 0;
      var dir = wheelAcc < 0 ? 1 : -1;
      wheelAcc = 0;
      return dir;
    }
    li.addEventListener('wheel', function (e) {
      if (!e.deltaY) return;
      // Once the list overflows, plain wheel keeps scrolling it — only the
      // slider cluster still owns the wheel then.
      var scroller = li.closest('.device-scroll');
      if (scroller && scroller.scrollHeight > scroller.clientHeight &&
          !(e.target.closest && e.target.closest('.dev-volume'))) return;
      e.preventDefault();
      var step = wheelStep(e);
      if (step) {
        draggingId = d.id;
        // slider.value is the drag-guarded source of truth; devices[] may
        // hold a lagging backend echo mid-session.
        var pct = Math.max(0, Math.min(100, Number(slider.value) + step * WHEEL_STEP));
        var dev = findDevice(d.id);
        if (dev) dev.volume = pct / 100;
        slider.value = pct;
        slider.style.setProperty('--val', pct);
        li.querySelector('.pct').value = pct + '%';
        pushVolume(pct);
      }
      clearTimeout(wheelEndTimer);
      wheelEndTimer = setTimeout(function () {
        wheelEndTimer = null;
        if (draggingId === d.id) draggingId = null;
        if (!findDevice(d.id)) return; // device vanished mid-wheel
        volumeTouched.set(d.id, Date.now());
        api.setVolume(d.id, Number(slider.value) / 100).catch(surfaceError);
      }, 250);
    }, { passive: false });

    return li;
  }

  function updateRow(li, d) {
    li.dataset.enabled = String(d.enabled);
    li.dataset.muted = String(d.muted);
    li.dataset.master = String(d.deviceType === 'master');

    var icon = li.querySelector('.dev-icon');
    icon.title = TYPE_LABELS[d.deviceType] || '';
    if (li.dataset.type !== d.deviceType) {
      li.dataset.type = d.deviceType;
      icon.innerHTML = ICONS[d.deviceType] || ICONS.speakers;
    }

    var name = li.querySelector('.dev-name');
    name.textContent = d.name;
    name.title = d.name;

    var mute = li.querySelector('.mute-btn');
    mute.setAttribute('aria-pressed', String(d.muted));
    mute.setAttribute('aria-label', (d.muted ? 'Unmute ' : 'Mute ') + d.name);
    mute.disabled = false;

    // Volume is adjustable regardless of enabled/muted state: it changes
    // the device's stored level without touching routing or mute.
    var slider = li.querySelector('input[type="range"]');
    slider.disabled = false;
    slider.setAttribute('aria-label', 'Volume for ' + d.name);
    var touched = volumeTouched.get(d.id) || 0;
    if (draggingId !== d.id && Date.now() - touched >= VOLUME_ECHO_GRACE_MS) {
      var pct = volumePct(d);
      slider.value = pct;
      slider.style.setProperty('--val', pct);
      li.querySelector('.pct').value = pct + '%';
    }

    var sw = li.querySelector('.switch');
    sw.setAttribute('aria-checked', String(d.enabled));
    sw.setAttribute('aria-label', 'Play audio on ' + d.name);
  }

  /* Keyed in-place render: preserves focus and slider drags. */
  function render() {
    var seen = new Set();
    devices.forEach(function (d) {
      seen.add(d.id);
      var li = rows.get(d.id);
      if (!li) {
        li = createRow(d);
        rows.set(d.id, li);
        list.appendChild(li);
      }
      updateRow(li, d);
    });
    rows.forEach(function (li, id) {
      if (!seen.has(id)) {
        if (draggingId === id) draggingId = null;
        li.remove();
        rows.delete(id);
      }
    });
    // Reconcile DOM order with backend order, moving only misplaced rows —
    // re-appending an already-placed node would blur focus and kill an
    // in-progress slider drag.
    var cursor = list.firstElementChild;
    devices.forEach(function (d) {
      var li = rows.get(d.id);
      if (li === cursor) {
        cursor = cursor.nextElementSibling;
      } else {
        list.insertBefore(li, cursor);
      }
    });
    updateStatus();
  }

  function findDevice(id) {
    return devices.find(function (d) { return d.id === id; }) || null;
  }

  function applyDeviceUpdate(updated) {
    if (!updated) return;
    devices = devices.map(function (d) { return d.id === updated.id ? updated : d; });
    render();
  }

  function setDevices(next) {
    if (!Array.isArray(next)) return;
    devices = next;
    render();
  }

  /* ================= errors & refetch ================= */

  /* Flash an error in the status bar, then restore the derived status. */
  var errorFlashTimer = null;
  function surfaceError(err) {
    console.error('backend error:', err);
    var msg = String(err && err.message ? err.message : err);
    if (msg.length > 90) msg = msg.slice(0, 87) + '…';
    statusbar.dataset.state = 'error';
    statusText.textContent = msg;
    clearTimeout(errorFlashTimer);
    errorFlashTimer = setTimeout(function () {
      errorFlashTimer = null;
      updateStatus();
    }, 5000);
  }

  /* A mutation failed after an optimistic update: surface it, then pull
     the authoritative state so the UI never shows routing that isn't real. */
  function revertOnError(err) {
    surfaceError(err);
    api.getDevices().then(setDevices).catch(function () {
      setDevices([]);
    });
  }

  function refresh() {
    api.refreshDevices().then(setDevices).catch(function (err) {
      setDevices([]);
      surfaceError(err);
    });
  }

  /* Leading + trailing throttle; the trailing call fires with the latest
     arg, so a burst never ends on a stale value. */
  function throttle(fn, ms) {
    var last = 0;
    var timer = null;
    var pending = null;
    return function (arg) {
      pending = arg;
      var now = Date.now();
      var run = function () {
        last = Date.now();
        timer = null;
        fn(pending);
      };
      if (now - last >= ms) {
        run();
      } else if (!timer) {
        timer = setTimeout(run, ms - (now - last));
      }
    };
  }

  /* ================= toolbar ================= */
  // No toolbar refresh: the backend monitor pushes every change on its own;
  // the empty state keeps a manual refresh as the recovery path.
  document.getElementById('emptyRefreshBtn').addEventListener('click', refresh);
  document.getElementById('selectAllBtn').addEventListener('click', function () {
    devices = devices.map(function (d) { return Object.assign({}, d, { enabled: true }); });
    render();
    api.setAllEnabled(true).catch(revertOnError);
  });
  document.getElementById('deselectAllBtn').addEventListener('click', function () {
    devices = devices.map(function (d) { return Object.assign({}, d, { enabled: false }); });
    render();
    api.setAllEnabled(false).catch(revertOnError);
  });

  /* ================= theme (set from the settings dialog) ================= */
  var MODES = ['system', 'light', 'dark'];
  function applyTheme(mode, persist) {
    if (MODES.indexOf(mode) === -1) mode = 'system';
    if (mode === 'system') document.documentElement.removeAttribute('data-theme');
    else document.documentElement.setAttribute('data-theme', mode);
    var radio = document.querySelector('input[name="theme"][value="' + mode + '"]');
    if (radio) radio.checked = true;
    if (persist) {
      try { localStorage.setItem('theme', mode); } catch (e) { /* storage unavailable */ }
    }
  }
  document.querySelectorAll('input[name="theme"]').forEach(function (r) {
    r.addEventListener('change', function () { applyTheme(r.value, true); });
  });

  /* ================= settings dialog ================= */
  var dialog = document.getElementById('settingsDialog');
  document.getElementById('settingsBtn').addEventListener('click', function () { dialog.showModal(); });
  document.getElementById('settingsClose').addEventListener('click', function () { dialog.close(); });
  document.getElementById('settingsCloseX').addEventListener('click', function () { dialog.close(); });
  dialog.addEventListener('click', function (e) { if (e.target === dialog) dialog.close(); });

  var syncSw = document.getElementById('syncSwitch');
  syncSw.addEventListener('click', function () {
    var on = syncSw.getAttribute('aria-checked') !== 'true';
    syncSw.setAttribute('aria-checked', String(on));
    try { localStorage.setItem('syncCompensation', String(on)); } catch (e) { /* ignore */ }
  });

  /* ================= init ================= */
  var savedTheme = null;
  var savedSync = null;
  try {
    savedTheme = localStorage.getItem('theme');
    savedSync = localStorage.getItem('syncCompensation');
  } catch (e) { /* storage unavailable */ }
  applyTheme(savedTheme || 'system', false);
  if (savedSync === 'true') syncSw.setAttribute('aria-checked', 'true');

  // Subscribe before the initial fetch so a change racing it is not missed;
  // both paths carry full state, so whichever lands last wins harmlessly.
  api.onDevicesChanged(setDevices);
  api.onBackendError(surfaceError);

  api.getDevices().then(setDevices).catch(function (err) {
    setDevices([]);
    surfaceError(err);
  });
})();
