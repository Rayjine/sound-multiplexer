/* Test harness: boots the REAL ui/index.html and executes the REAL ui/main.js
   inside jsdom. With no window.__TAURI__ the app's built-in mock backend
   activates; pass a fake TAURI object (see makeFakeTauri) to intercept
   invoke() calls instead. */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { JSDOM, VirtualConsole } from 'jsdom';

const uiDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', 'ui');
const html = readFileSync(path.join(uiDir, 'index.html'), 'utf8');
const mainJs = readFileSync(path.join(uiDir, 'main.js'), 'utf8');

/**
 * Load the app in a fresh jsdom window.
 *
 * @param {import('node:test').TestContext} t - test context; the window is
 *   closed in t.after() so pending app timers never keep the process alive.
 * @param {object} [options]
 * @param {object} [options.tauri] - installed as window.__TAURI__ BEFORE main.js runs.
 * @param {(window: object) => void} [options.beforeMain] - prepare the window
 *   (e.g. seed localStorage) before main.js runs.
 * @returns {Window} the jsdom window, with main.js already executed.
 */
export function loadApp(t, options = {}) {
  // Quiet console: the <script src="main.js"> tag in index.html is inert under
  // runScripts: 'outside-only' and must not spam resource-loading errors.
  const virtualConsole = new VirtualConsole();
  virtualConsole.on('jsdomError', () => {});

  const dom = new JSDOM(html, {
    url: 'http://localhost/',
    runScripts: 'outside-only',
    virtualConsole,
  });
  const { window } = dom;

  // jsdom gaps: <dialog> modal methods and matchMedia may be missing.
  const dialogProto = window.HTMLDialogElement && window.HTMLDialogElement.prototype;
  if (dialogProto && typeof dialogProto.showModal !== 'function') {
    dialogProto.showModal = function () { this.setAttribute('open', ''); };
    dialogProto.close = function () { this.removeAttribute('open'); };
  }
  if (typeof window.matchMedia !== 'function') {
    window.matchMedia = () => ({
      matches: false,
      media: '',
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
      removeListener() {},
    });
  }

  if (options.tauri) window.__TAURI__ = options.tauri;
  if (options.beforeMain) options.beforeMain(window);

  window.eval(mainJs);
  t.after(() => window.close());
  return window;
}

/** Wait one macrotask (lets window timers scheduled earlier fire first). */
export const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

/** Let the app settle: initial getDevices resolution plus queued emits/reverts. */
export async function settle() {
  await tick();
  await tick();
}

/** All rendered device rows, in DOM order. */
export function rows(document) {
  return Array.from(document.querySelectorAll('#deviceList > .device'));
}

/** The rendered row for a device id. */
export function rowById(document, id) {
  return document.querySelector('#deviceList > .device[data-id="' + id + '"]');
}

/** Device fixture matching the Rust `Device` serde shape. */
export function device(id, overrides = {}) {
  return {
    id,
    name: 'Device ' + id,
    deviceType: 'speakers',
    enabled: false,
    volume: 0.5,
    muted: false,
    ...overrides,
  };
}

/**
 * A fake window.__TAURI__ that records every invoke() and lets tests fire
 * backend events. `handlers` maps command name -> (args) => result; a thrown
 * value or rejected promise becomes the invoke rejection, like real Tauri.
 */
export function makeFakeTauri(handlers) {
  const calls = [];
  const listeners = new Map();
  const tauri = {
    core: {
      invoke(cmd, args) {
        // Clone args into this realm: main.js builds them inside the jsdom
        // window, whose Object.prototype fails deepStrictEqual across realms.
        calls.push({ cmd, args: args === undefined ? undefined : { ...args } });
        const handler = handlers[cmd];
        if (!handler) return Promise.reject('no fake handler for ' + cmd);
        try {
          return Promise.resolve(handler(args));
        } catch (err) {
          return Promise.reject(err);
        }
      },
    },
    event: {
      listen(name, cb) {
        listeners.set(name, cb);
        return Promise.resolve(() => {});
      },
    },
  };
  return {
    tauri,
    calls,
    callsFor: (cmd) => calls.filter((c) => c.cmd === cmd),
    emit(name, payload) {
      const cb = listeners.get(name);
      if (cb) cb({ payload });
    },
  };
}

/** Count list.insertBefore() calls (row moves) made while fn runs. */
export function countMoves(list, fn) {
  const proto = Object.getPrototypeOf(list);
  let moves = 0;
  list.insertBefore = function (...args) {
    moves += 1;
    return proto.insertBefore.apply(this, args);
  };
  try {
    fn();
  } finally {
    delete list.insertBefore;
  }
  return moves;
}
