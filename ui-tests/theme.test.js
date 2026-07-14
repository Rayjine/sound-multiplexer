/* Theme selection and persistence. The settings-dialog radios are the only
   way to change theme — the titlebar toggle button was removed. */

import test from 'node:test';
import assert from 'node:assert/strict';
import { loadApp } from './helpers/harness.js';

function checkedTheme(document) {
  return document.querySelector('input[name="theme"]:checked').value;
}

function pick(window, id) {
  const radio = window.document.getElementById(id);
  radio.checked = true;
  radio.dispatchEvent(new window.Event('change', { bubbles: true }));
}

test('picking theme radios applies and persists each mode', async (t) => {
  const window = loadApp(t);
  const document = window.document;
  const root = document.documentElement;

  assert.equal(root.hasAttribute('data-theme'), false);
  assert.equal(checkedTheme(document), 'system');

  pick(window, 'themeLight');
  assert.equal(root.getAttribute('data-theme'), 'light');
  assert.equal(window.localStorage.getItem('theme'), 'light');

  pick(window, 'themeDark');
  assert.equal(root.getAttribute('data-theme'), 'dark');
  assert.equal(window.localStorage.getItem('theme'), 'dark');

  pick(window, 'themeSystem');
  assert.equal(root.hasAttribute('data-theme'), false, 'system mode removes data-theme');
  assert.equal(window.localStorage.getItem('theme'), 'system');
});

test('saved theme is restored on startup and reflected in the radios', async (t) => {
  const window = loadApp(t, {
    beforeMain: (w) => w.localStorage.setItem('theme', 'dark'),
  });
  const document = window.document;

  assert.equal(document.documentElement.getAttribute('data-theme'), 'dark');
  assert.equal(checkedTheme(document), 'dark');
});

test('an unknown saved theme falls back to system', async (t) => {
  const window = loadApp(t, {
    beforeMain: (w) => w.localStorage.setItem('theme', 'neon'),
  });
  const document = window.document;

  assert.equal(document.documentElement.hasAttribute('data-theme'), false);
  assert.equal(checkedTheme(document), 'system');
});

test('the titlebar has no theme toggle', async (t) => {
  const window = loadApp(t);
  assert.equal(window.document.getElementById('themeToggle'), null);
});
