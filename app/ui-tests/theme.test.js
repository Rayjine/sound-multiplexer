/* Theme cycling, persistence to localStorage, and settings-radio sync. */

import test from 'node:test';
import assert from 'node:assert/strict';
import { loadApp } from './helpers/harness.js';

function checkedTheme(document) {
  return document.querySelector('input[name="theme"]:checked').value;
}

test('theme toggle cycles system -> light -> dark -> system, persisting each step', async (t) => {
  const window = loadApp(t);
  const document = window.document;
  const root = document.documentElement;
  const toggle = document.getElementById('themeToggle');
  const label = document.getElementById('themeLabel');

  assert.equal(root.hasAttribute('data-theme'), false);
  assert.equal(label.textContent, 'System');
  assert.equal(checkedTheme(document), 'system');

  toggle.click();
  assert.equal(root.getAttribute('data-theme'), 'light');
  assert.equal(window.localStorage.getItem('theme'), 'light');
  assert.equal(label.textContent, 'Light');
  assert.equal(checkedTheme(document), 'light');

  toggle.click();
  assert.equal(root.getAttribute('data-theme'), 'dark');
  assert.equal(window.localStorage.getItem('theme'), 'dark');
  assert.equal(label.textContent, 'Dark');
  assert.equal(checkedTheme(document), 'dark');

  toggle.click();
  assert.equal(root.hasAttribute('data-theme'), false, 'system mode removes data-theme');
  assert.equal(window.localStorage.getItem('theme'), 'system');
  assert.equal(label.textContent, 'System');
  assert.equal(checkedTheme(document), 'system');
});

test('saved theme is restored on startup', async (t) => {
  const window = loadApp(t, {
    beforeMain: (w) => w.localStorage.setItem('theme', 'dark'),
  });
  const document = window.document;

  assert.equal(document.documentElement.getAttribute('data-theme'), 'dark');
  assert.equal(document.getElementById('themeLabel').textContent, 'Dark');
  assert.equal(checkedTheme(document), 'dark');
});

test('an unknown saved theme falls back to system', async (t) => {
  const window = loadApp(t, {
    beforeMain: (w) => w.localStorage.setItem('theme', 'neon'),
  });
  const document = window.document;

  assert.equal(document.documentElement.hasAttribute('data-theme'), false);
  assert.equal(document.getElementById('themeLabel').textContent, 'System');
  assert.equal(checkedTheme(document), 'system');
});

test('picking a theme radio in settings applies and persists it', async (t) => {
  const window = loadApp(t);
  const document = window.document;
  const light = document.getElementById('themeLight');

  light.checked = true;
  light.dispatchEvent(new window.Event('change', { bubbles: true }));

  assert.equal(document.documentElement.getAttribute('data-theme'), 'light');
  assert.equal(window.localStorage.getItem('theme'), 'light');
  assert.equal(document.getElementById('themeLabel').textContent, 'Light');
});
