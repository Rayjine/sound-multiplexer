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
