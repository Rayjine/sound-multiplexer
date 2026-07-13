/* Initial render against the app's built-in mock backend (no __TAURI__),
   plus empty-list and failed-load states via a fake __TAURI__. */

import test from 'node:test';
import assert from 'node:assert/strict';
import { loadApp, settle, rows, rowById, makeFakeTauri } from './helpers/harness.js';

// The built-in mock devices hardcoded in ui/main.js.
const MOCK = [
  { id: 'mock.analog', name: 'Built-in Audio Analog Stereo', type: 'speakers', title: 'Speakers', enabled: true, pct: 65, muted: false },
  { id: 'mock.hp', name: 'HD 560S Analog Stereo', type: 'headphones', title: 'Headphones', enabled: true, pct: 40, muted: false },
  { id: 'mock.bt', name: 'WH-1000XM5', type: 'bluetooth', title: 'Bluetooth', enabled: false, pct: 80, muted: false },
  { id: 'mock.hdmi', name: 'Navi 21/23 HDMI Audio [Radeon RX 6800]', type: 'hdmi', title: 'HDMI / display audio', enabled: false, pct: 100, muted: true },
];

test('initial render: one row per mock device with name and type icon', async (t) => {
  const window = loadApp(t);
  await settle();
  const document = window.document;

  const lis = rows(document);
  assert.equal(lis.length, MOCK.length);
  assert.deepEqual(lis.map((li) => li.dataset.id), MOCK.map((d) => d.id));
  assert.deepEqual(
    lis.map((li) => li.querySelector('.dev-name').textContent),
    MOCK.map((d) => d.name),
  );
  assert.deepEqual(lis.map((li) => li.dataset.type), MOCK.map((d) => d.type));
  assert.deepEqual(
    lis.map((li) => li.querySelector('.dev-icon').title),
    MOCK.map((d) => d.title),
  );
  // Each device type renders its own distinct SVG icon.
  const icons = lis.map((li) => li.querySelector('.dev-icon').innerHTML);
  icons.forEach((markup) => assert.match(markup, /<svg/));
  assert.equal(new Set(icons).size, MOCK.length);
});

test('initial render: volume, mute and enabled state per row', async (t) => {
  const window = loadApp(t);
  await settle();
  const document = window.document;

  for (const d of MOCK) {
    const li = rowById(document, d.id);
    const slider = li.querySelector('input[type="range"]');
    const mute = li.querySelector('.mute-btn');
    const sw = li.querySelector('.switch');

    assert.equal(li.dataset.enabled, String(d.enabled), d.id + ' enabled');
    assert.equal(sw.getAttribute('aria-checked'), String(d.enabled), d.id + ' switch');
    assert.equal(li.dataset.muted, String(d.muted), d.id + ' muted');
    assert.equal(mute.getAttribute('aria-pressed'), String(d.muted), d.id + ' mute btn');
    assert.equal(slider.value, String(d.pct), d.id + ' slider value');
    assert.equal(slider.style.getPropertyValue('--val'), String(d.pct), d.id + ' --val');
    assert.equal(li.querySelector('.pct').textContent, d.pct + '%', d.id + ' readout');
    // Slider is usable only on enabled, unmuted devices; mute only on enabled ones.
    assert.equal(slider.disabled, !d.enabled || d.muted, d.id + ' slider disabled');
    assert.equal(mute.disabled, !d.enabled, d.id + ' mute disabled');
  }

  assert.equal(document.getElementById('app').dataset.empty, 'false');
  assert.equal(document.getElementById('statusbar').dataset.state, 'active');
  assert.equal(document.getElementById('statusText').textContent, '2 devices active');
});

test('empty device list -> data-empty=true and error status', async (t) => {
  const fake = makeFakeTauri({ get_devices: () => [] });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;

  assert.equal(rows(document).length, 0);
  assert.equal(document.getElementById('app').dataset.empty, 'true');
  assert.equal(document.getElementById('statusbar').dataset.state, 'error');
  assert.equal(document.getElementById('statusText').textContent, 'No output devices found');
});

test('failed initial load -> empty state with the backend error surfaced', async (t) => {
  const fake = makeFakeTauri({
    get_devices: () => { throw 'PipeWire connection lost'; },
  });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;

  assert.equal(rows(document).length, 0);
  assert.equal(document.getElementById('app').dataset.empty, 'true');
  assert.equal(document.getElementById('statusbar').dataset.state, 'error');
  assert.equal(document.getElementById('statusText').textContent, 'PipeWire connection lost');
});
