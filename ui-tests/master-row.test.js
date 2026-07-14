/* Master volume row: the synthetic combined-output device the backend
   prepends while 2+ devices are enabled. Driven via a fake __TAURI__ so the
   tests control the exact payload the backend would send. */

import test from 'node:test';
import assert from 'node:assert/strict';
import { loadApp, settle, rows, rowById, device, makeFakeTauri } from './helpers/harness.js';

const MASTER_ID = 'sound_multiplexer_combined';

const masterDevice = (overrides = {}) => ({
  id: MASTER_ID,
  name: 'Master volume',
  deviceType: 'master',
  enabled: true,
  volume: 0.8,
  muted: false,
  ...overrides,
});

/* Backend contract: the master row is PREPENDED to the real devices. */
const withMaster = () => [
  masterDevice(),
  device('dev.a', { enabled: true }),
  device('dev.b', { enabled: true, deviceType: 'headphones' }),
];

test('master row renders first with data-master, brand icon and its real state', async (t) => {
  const fake = makeFakeTauri({ get_devices: withMaster });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;

  const lis = rows(document);
  assert.deepEqual(lis.map((li) => li.dataset.id), [MASTER_ID, 'dev.a', 'dev.b'],
    'master row must lead the list');

  const master = lis[0];
  assert.equal(master.dataset.master, 'true');
  assert.equal(master.dataset.type, 'master');
  assert.equal(master.querySelector('.dev-name').textContent, 'Master volume');

  // Brand-mark icon: the app's own three-bar logo, not a device-type icon.
  const icon = master.querySelector('.dev-icon');
  assert.match(icon.innerHTML, /<svg/);
  assert.equal(icon.querySelectorAll('rect').length, 3, 'brand mark has three bars');
  assert.equal(icon.title, 'Combined output — controls all enabled devices at once');

  // The enable switch is present in the DOM but hidden by the
  // .device[data-master="true"] .switch CSS rule; jsdom does not compute
  // styles, so data-master IS the hiding contract asserted here.
  assert.ok(master.querySelector('.switch'), 'switch element exists (CSS-hidden)');

  // The row reflects the combine sink's real state like any other row.
  assert.equal(master.dataset.enabled, 'true');
  assert.equal(master.dataset.muted, 'false');
  assert.equal(master.querySelector('input[type="range"]').value, '80');
  assert.equal(master.querySelector('.pct').textContent, '80%');

  // Real device rows are not master rows.
  assert.equal(lis[1].dataset.master, 'false');
  assert.equal(lis[2].dataset.master, 'false');
});

test('master row is excluded from the status count and the empty check', async (t) => {
  const fake = makeFakeTauri({ get_devices: withMaster });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;

  // Three rows rendered, but only the two REAL devices count as active.
  assert.equal(rows(document).length, 3);
  assert.equal(document.getElementById('statusText').textContent, '2 devices active');
  assert.equal(document.getElementById('statusbar').dataset.state, 'active');
  assert.equal(document.getElementById('app').dataset.empty, 'false');
});

test('a payload with only a master device (edge case) yields the empty state', async (t) => {
  const fake = makeFakeTauri({ get_devices: withMaster });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;
  assert.equal(document.getElementById('app').dataset.empty, 'false');

  // All real devices vanish, only the master row remains in the payload:
  // the empty check counts real devices only, so this IS the empty state.
  fake.emit('devices-changed', [masterDevice()]);

  assert.equal(document.getElementById('app').dataset.empty, 'true');
  assert.equal(document.getElementById('statusbar').dataset.state, 'error');
  assert.equal(document.getElementById('statusText').textContent, 'No output devices found');
});

test('master slider input invokes set_device_volume with the combined sink id', async (t) => {
  const fake = makeFakeTauri({
    get_devices: withMaster,
    set_device_volume: () => null,
  });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();

  const master = rowById(window.document, MASTER_ID);
  const slider = master.querySelector('input[type="range"]');
  assert.equal(slider.disabled, false);
  slider.value = '45';
  slider.dispatchEvent(new window.Event('input', { bubbles: true }));

  assert.deepEqual(fake.callsFor('set_device_volume'), [
    { cmd: 'set_device_volume', args: { id: MASTER_ID, volume: 0.45 } },
  ]);
  assert.equal(fake.callsFor('set_device_enabled').length, 0,
    'master volume must never touch routing');
});
