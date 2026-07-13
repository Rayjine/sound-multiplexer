/* User interactions against the app's built-in mock backend (no __TAURI__):
   switch toggles, mute, volume slider, select/deselect all. */

import test from 'node:test';
import assert from 'node:assert/strict';
import { loadApp, settle, rows, rowById } from './helpers/harness.js';

test('switch click optimistically flips data-enabled and the status count', async (t) => {
  const window = loadApp(t);
  await settle();
  const document = window.document;
  const bt = rowById(document, 'mock.bt');
  const statusText = document.getElementById('statusText');

  assert.equal(bt.dataset.enabled, 'false');
  bt.querySelector('.switch').click();

  // Assert synchronously: the flip must land before the backend confirms.
  assert.equal(bt.dataset.enabled, 'true');
  assert.equal(bt.querySelector('.switch').getAttribute('aria-checked'), 'true');
  assert.equal(statusText.textContent, '3 devices active');
  assert.equal(document.getElementById('statusbar').dataset.state, 'active');

  // Backend confirmation must not change anything.
  await settle();
  assert.equal(bt.dataset.enabled, 'true');
  assert.equal(statusText.textContent, '3 devices active');
});

test('status wording: singular count, then silent mode when nothing is enabled', async (t) => {
  const window = loadApp(t);
  await settle();
  const document = window.document;
  const statusbar = document.getElementById('statusbar');
  const statusText = document.getElementById('statusText');

  rowById(document, 'mock.analog').querySelector('.switch').click();
  assert.equal(statusText.textContent, '1 device active');
  await settle();

  rowById(document, 'mock.hp').querySelector('.switch').click();
  assert.equal(statusbar.dataset.state, 'silent');
  assert.equal(statusText.textContent, 'Silent mode — no devices selected');
  await settle();
  assert.equal(statusText.textContent, 'Silent mode — no devices selected');
});

test('mute click shows muted hooks (data-muted, chip, aria) and disables the slider', async (t) => {
  const window = loadApp(t);
  await settle();
  const li = rowById(window.document, 'mock.analog');
  const mute = li.querySelector('.mute-btn');
  const slider = li.querySelector('input[type="range"]');

  assert.equal(li.dataset.muted, 'false');
  assert.equal(slider.disabled, false);

  mute.click();
  assert.equal(li.dataset.muted, 'true');
  assert.equal(mute.getAttribute('aria-pressed'), 'true');
  assert.equal(mute.getAttribute('aria-label'), 'Unmute Built-in Audio Analog Stereo');
  assert.equal(li.querySelector('.muted-chip').textContent, 'Muted');
  assert.equal(slider.disabled, true);
  assert.equal(mute.disabled, false, 'mute stays clickable so the user can unmute');
  await settle();
  assert.equal(li.dataset.muted, 'true');

  mute.click();
  assert.equal(li.dataset.muted, 'false');
  assert.equal(mute.getAttribute('aria-pressed'), 'false');
  assert.equal(slider.disabled, false);
});

test('slider input updates the % readout and the --val custom property', async (t) => {
  const window = loadApp(t);
  await settle();
  const li = rowById(window.document, 'mock.hp');
  const slider = li.querySelector('input[type="range"]');

  slider.value = '55';
  slider.dispatchEvent(new window.Event('input', { bubbles: true }));
  assert.equal(li.querySelector('.pct').textContent, '55%');
  assert.equal(slider.style.getPropertyValue('--val'), '55');

  slider.dispatchEvent(new window.Event('change', { bubbles: true }));
  await settle();
  assert.equal(slider.value, '55');
  assert.equal(li.querySelector('.pct').textContent, '55%');
});

test('select all / deselect all update every row and the status line', async (t) => {
  const window = loadApp(t);
  await settle();
  const document = window.document;
  const statusbar = document.getElementById('statusbar');
  const statusText = document.getElementById('statusText');

  document.getElementById('selectAllBtn').click();
  for (const li of rows(document)) {
    assert.equal(li.dataset.enabled, 'true');
    assert.equal(li.querySelector('.switch').getAttribute('aria-checked'), 'true');
  }
  assert.equal(statusbar.dataset.state, 'active');
  assert.equal(statusText.textContent, '4 devices active');
  await settle();
  assert.equal(statusText.textContent, '4 devices active');

  document.getElementById('deselectAllBtn').click();
  for (const li of rows(document)) {
    assert.equal(li.dataset.enabled, 'false');
    assert.equal(li.querySelector('.switch').getAttribute('aria-checked'), 'false');
  }
  assert.equal(statusbar.dataset.state, 'silent');
  assert.equal(statusText.textContent, 'Silent mode — no devices selected');
  assert.equal(document.getElementById('app').dataset.empty, 'false');
  await settle();
  assert.equal(statusText.textContent, 'Silent mode — no devices selected');
});
