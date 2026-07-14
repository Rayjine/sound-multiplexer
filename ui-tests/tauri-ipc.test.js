/* Fake-__TAURI__ variant: assert the exact invoke() payloads and the
   revert-on-error path (refetch + error flash) after a rejected invoke. */

import test from 'node:test';
import assert from 'node:assert/strict';
import { loadApp, settle, rowById, device, makeFakeTauri } from './helpers/harness.js';

test('switch click invokes set_device_enabled with the exact payload', async (t) => {
  const fake = makeFakeTauri({
    get_devices: () => [device('dev.a', { enabled: true }), device('dev.b')],
    set_device_enabled: () => null,
  });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;

  rowById(document, 'dev.b').querySelector('.switch').click();
  assert.deepEqual(fake.callsFor('set_device_enabled'), [
    { cmd: 'set_device_enabled', args: { id: 'dev.b', enabled: true } },
  ]);

  rowById(document, 'dev.a').querySelector('.switch').click();
  assert.deepEqual(fake.callsFor('set_device_enabled')[1], {
    cmd: 'set_device_enabled',
    args: { id: 'dev.a', enabled: false },
  });
});

test('select all / deselect all invoke set_all_enabled with the exact payload', async (t) => {
  const fake = makeFakeTauri({
    get_devices: () => [device('dev.a'), device('dev.b')],
    set_all_enabled: () => null,
  });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();

  window.document.getElementById('selectAllBtn').click();
  window.document.getElementById('deselectAllBtn').click();
  assert.deepEqual(fake.callsFor('set_all_enabled'), [
    { cmd: 'set_all_enabled', args: { enabled: true } },
    { cmd: 'set_all_enabled', args: { enabled: false } },
  ]);
});

test('slider input invokes set_device_volume with a 0..1 volume', async (t) => {
  const fake = makeFakeTauri({
    get_devices: () => [device('dev.a', { enabled: true })],
    set_device_volume: () => null,
  });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();

  const slider = rowById(window.document, 'dev.a').querySelector('input[type="range"]');
  slider.value = '55';
  slider.dispatchEvent(new window.Event('input', { bubbles: true }));
  assert.deepEqual(fake.callsFor('set_device_volume'), [
    { cmd: 'set_device_volume', args: { id: 'dev.a', volume: 0.55 } },
  ]);
});

test('slider input on a disabled or muted device sets only the volume', async (t) => {
  const fake = makeFakeTauri({
    get_devices: () => [device('dev.off'), device('dev.muted', { enabled: true, muted: true })],
    set_device_volume: () => null,
    set_device_enabled: () => null,
    set_device_muted: () => null,
  });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;

  // Disabled device: the slider stays interactive and moving it invokes
  // set_device_volume only — the enabled set must not change.
  const offRow = rowById(document, 'dev.off');
  const offSlider = offRow.querySelector('input[type="range"]');
  assert.equal(offSlider.disabled, false, 'disabled device keeps an interactive slider');
  offSlider.value = '30';
  offSlider.dispatchEvent(new window.Event('input', { bubbles: true }));
  assert.deepEqual(fake.callsFor('set_device_volume'), [
    { cmd: 'set_device_volume', args: { id: 'dev.off', volume: 0.3 } },
  ]);
  assert.equal(fake.callsFor('set_device_enabled').length, 0,
    'volume on a disabled device must not invoke set_device_enabled');
  assert.equal(offRow.dataset.enabled, 'false', 'row stays disabled');

  // Muted device: same contract, and the mute state is untouched.
  const mutedRow = rowById(document, 'dev.muted');
  const mutedSlider = mutedRow.querySelector('input[type="range"]');
  assert.equal(mutedSlider.disabled, false, 'muted device keeps an interactive slider');
  mutedSlider.value = '70';
  mutedSlider.dispatchEvent(new window.Event('input', { bubbles: true }));
  assert.deepEqual(fake.callsFor('set_device_volume')[1], {
    cmd: 'set_device_volume',
    args: { id: 'dev.muted', volume: 0.7 },
  });
  assert.equal(fake.callsFor('set_device_muted').length, 0,
    'volume on a muted device must not invoke set_device_muted');
  assert.equal(mutedRow.dataset.muted, 'true', 'row stays muted');
});

test('rejected set_device_enabled refetches devices, reverts the row and flashes the error', async (t) => {
  const fake = makeFakeTauri({
    // Authoritative state: dev.a stays disabled.
    get_devices: () => [device('dev.a', { name: 'Living Room DAC' })],
    set_device_enabled: () => { throw 'routing failed: sink is busy'; },
  });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  const document = window.document;
  const row = rowById(document, 'dev.a');

  row.querySelector('.switch').click();
  assert.equal(row.dataset.enabled, 'true', 'optimistic flip lands first');

  await settle();
  assert.equal(fake.callsFor('get_devices').length, 2, 'failure must trigger an authoritative refetch');
  assert.equal(row.dataset.enabled, 'false', 'row reverts to backend state');
  assert.equal(row.querySelector('.switch').getAttribute('aria-checked'), 'false');
  assert.equal(rowById(document, 'dev.a') === row, true, 'revert reuses the same row element');
  assert.equal(document.getElementById('statusbar').dataset.state, 'error');
  assert.equal(document.getElementById('statusText').textContent, 'routing failed: sink is busy');
});
