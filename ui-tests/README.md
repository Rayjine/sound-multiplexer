# UI tests

DOM-level tests for the frontend. Each test boots the **real** `ui/index.html`
and executes the **real** `ui/main.js` inside jsdom — nothing is bundled or
stubbed out of the app itself, so what passes here is the exact code the Tauri
webview runs.

## Running

```
npm install   # once; jsdom is the only dependency
npm test      # node --test, Node 20+ (CI uses 22)
```

Not part of `cargo test`; CI runs it as a separate step.

## Harness (`helpers/harness.js`)

- `loadApp(t)` — fresh jsdom window per test. With no `window.__TAURI__`,
  main.js activates its in-memory mock backend; use this when only rendering
  or interaction behavior matters.
- `loadApp(t, { tauri: makeFakeTauri(handlers).tauri })` — installs a fake
  `__TAURI__` *before* main.js runs. It records every `invoke()` (assert exact
  command payloads via `callsFor`), maps command name → handler (throw/reject
  to simulate backend failure), and `emit()` fires `devices-changed` /
  `backend-error` with a payload of your choosing.
- jsdom gaps are stubbed: `<dialog>.showModal/close`, `window.matchMedia`.
- `settle()` waits two macrotasks so the initial fetch and queued emits land;
  `countMoves()` counts `insertBefore` calls to assert minimal-move reordering.

## Coverage

`render` (initial render, empty state, failed load), `reconciliation` (keyed
row reuse, minimal-move reorder, removal), `tauri-ipc` (command payloads,
optimistic revert on rejection), `interactions` (switch/mute/slider/bulk,
status wording), `master-row` (synthetic combined-output row contract),
`theme` (cycling + persistence).

## Adding a test

Create `<topic>.test.js` next to the others (`node --test` picks up any
`*.test.js`). Copy the shape of an existing test: `loadApp(t, …)`, `await
settle()`, assert on `window.document`. Reach for `makeFakeTauri` whenever the
test cares about payloads in either direction; device fixtures come from
`device(id, overrides)`, which matches the Rust `Device` serde shape.
