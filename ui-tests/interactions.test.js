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

test('mute click shows muted hooks (data-muted, aria) and keeps the slider interactive', async (t) => {
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
  assert.equal(slider.disabled, false, 'muted row keeps an interactive slider');
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

function wheel(window, target, deltaY, deltaMode = 0) {
  target.dispatchEvent(new window.WheelEvent('wheel', { deltaY, deltaMode, bubbles: true, cancelable: true }));
}

test('a wheel notch on a row nudges the volume by 5%, clamped to 0-100', async (t) => {
  const window = loadApp(t);
  await settle();
  const document = window.document;
  const li = rowById(document, 'mock.analog'); // starts at 65%
  const slider = li.querySelector('input[type="range"]');

  wheel(window, li, -40); // one discrete notch up (webkit reports ~40px)
  assert.equal(slider.value, '70');
  assert.equal(li.querySelector('.pct').value, '70%');

  wheel(window, li, 40);
  assert.equal(slider.value, '65');

  wheel(window, li, -3, 1); // one notch in line mode (Firefox-style)
  assert.equal(slider.value, '70');

  const maxed = rowById(document, 'mock.hdmi'); // starts at 100%
  wheel(window, maxed, -40);
  assert.equal(maxed.querySelector('input[type="range"]').value, '100', 'clamped at the top');
});

test('fine touchpad deltas accumulate into a single step instead of one per event', async (t) => {
  const window = loadApp(t);
  await settle();
  const li = rowById(window.document, 'mock.analog'); // starts at 65%
  const slider = li.querySelector('input[type="range"]');

  wheel(window, li, -12);
  wheel(window, li, -12);
  assert.equal(slider.value, '65', 'below the accumulation threshold: no step yet');
  wheel(window, li, -12); // 36px accumulated -> one 5% step
  assert.equal(slider.value, '70');
});

test('the toolbar has no refresh button; the empty state keeps one', async (t) => {
  const window = loadApp(t);
  await settle();
  assert.equal(window.document.getElementById('refreshBtn'), null);
  assert.notEqual(window.document.getElementById('emptyRefreshBtn'), null);
});
