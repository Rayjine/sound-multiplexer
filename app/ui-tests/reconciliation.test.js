/* render() keyed reconciliation: devices-changed payloads must update rows
   in place (same elements), follow payload order, and move only misplaced
   rows. Uses a fake __TAURI__ so tests can fire devices-changed directly. */

import test from 'node:test';
import assert from 'node:assert/strict';
import { loadApp, settle, rows, device, makeFakeTauri, countMoves } from './helpers/harness.js';

const IDS = ['dev.a', 'dev.b', 'dev.c', 'dev.d'];
const initialDevices = () => [
  device('dev.a', { enabled: true }),
  device('dev.b', { deviceType: 'headphones' }),
  device('dev.c', { enabled: true, deviceType: 'bluetooth' }),
  device('dev.d', { deviceType: 'hdmi' }),
];

async function boot(t) {
  const fake = makeFakeTauri({ get_devices: initialDevices });
  const window = loadApp(t, { tauri: fake.tauri });
  await settle();
  return { fake, document: window.document };
}

test('devices-changed with the same ids reuses the exact row elements with zero moves', async (t) => {
  const { fake, document } = await boot(t);
  const before = rows(document);
  assert.deepEqual(before.map((li) => li.dataset.id), IDS);

  // Same ids, same order, one field changed -> in-place update, no DOM moves.
  const payload = initialDevices().map((d) => ({ ...d, volume: 0.25 }));
  const moves = countMoves(document.getElementById('deviceList'), () => {
    fake.emit('devices-changed', payload);
  });

  const after = rows(document);
  assert.equal(moves, 0, 'no row may be moved when the order is unchanged');
  assert.equal(after.length, before.length);
  before.forEach((li, i) => {
    assert.equal(after[i] === li, true, 'row ' + i + ' must be the identical element');
  });
  // ...and the changed field really was applied to the reused elements.
  for (const li of after) {
    assert.equal(li.querySelector('input[type="range"]').value, '25');
    assert.equal(li.querySelector('.pct').textContent, '25%');
  }
});

test('devices-changed reorder follows payload order with minimal moves', async (t) => {
  const { fake, document } = await boot(t);
  const byId = new Map(rows(document).map((li) => [li.dataset.id, li]));

  // Move the last device to the front: exactly one row is out of place.
  const payload = initialDevices();
  payload.unshift(payload.pop());
  const moves = countMoves(document.getElementById('deviceList'), () => {
    fake.emit('devices-changed', payload);
  });

  assert.equal(moves, 1, 'only the single misplaced row may be moved');
  assert.deepEqual(
    rows(document).map((li) => li.dataset.id),
    ['dev.d', 'dev.a', 'dev.b', 'dev.c'],
    'DOM order must follow payload order',
  );
  for (const li of rows(document)) {
    assert.equal(byId.get(li.dataset.id) === li, true, li.dataset.id + ' must be reused');
  }
});

test('devices-changed removal drops the row and keeps the survivors intact', async (t) => {
  const { fake, document } = await boot(t);
  const byId = new Map(rows(document).map((li) => [li.dataset.id, li]));

  fake.emit('devices-changed', initialDevices().filter((d) => d.id !== 'dev.b'));

  const after = rows(document);
  assert.deepEqual(after.map((li) => li.dataset.id), ['dev.a', 'dev.c', 'dev.d']);
  assert.equal(byId.get('dev.b').isConnected, false, 'removed row must leave the DOM');
  for (const li of after) {
    assert.equal(byId.get(li.dataset.id) === li, true, li.dataset.id + ' must be reused');
  }
});
