# Architecture

Sound Multiplexer plays system audio on several output devices at once. It is a
Rust workspace: a platform-audio crate (`audio/`) behind one trait, a thin Tauri
v2 shell (`src-tauri/`) that exposes that trait as IPC commands and a single
event stream, and a framework-free HTML/JS frontend (`ui/`). This document is
the deep reference; the [README](../README.md) stays short.

```
audio/                  platform backends (no Tauri dependency)
  src/lib.rs            AudioBackend trait, Device/DeviceType, BackendEvent
  src/linux.rs          PulseAudio/PipeWire backend (pactl)
  src/fanout.rs         OS-free ring buffer + format math for the Windows engine
  src/windows/mod.rs    MMDevice/WASAPI backend, IPolicyConfig, monitoring
  src/windows/engine.rs loopback fan-out engine (capture + render threads)
  tests/linux_live.rs   live E2E against a real sound server (opt-in)
src-tauri/
  src/lib.rs            commands, state, event pump
  src/main.rs           entry point (Linux WebKit workaround)
  tauri.conf.json       window, CSP, bundle targets
ui/                     index.html + styles.css (design tokens) + main.js
ui-tests/               DOM tests: the real ui/ files running in jsdom
.github/workflows/ci.yml
```

## The `AudioBackend` contract

`audio/src/lib.rs` defines the whole app-facing surface:

- `list_devices()` — current output devices, excluding sinks the app created
  itself; `enabled` reflects the currently applied set.
- `apply_enabled(ids)` — apply the full enabled set in one routing update.
  Unknown ids are ignored; must be idempotent (re-applying the same set must
  not rebuild anything audible).
- `set_volume` / `set_muted` — act on the physical device, 0.0..=1.0.
- `start_monitoring(tx)` — spawn the change monitor; callable repeatedly, each
  call replaces the previous monitor (used to revive monitoring after a sound
  server restart).
- `cleanup()` — tear down everything the app created; must leave the system on
  a sane default device.

`BackendEvent` is deliberately coarse: on `DevicesChanged` the app re-lists
everything and pushes full state; `VolumeChanged`/`MuteChanged` carry ids for
external changes; `Error` surfaces non-fatal problems to the UI.

Routing strategy by enabled-set size, both platforms:

| enabled | Linux (`linux.rs`)                          | Windows (`windows/`)                            |
|---------|---------------------------------------------|-------------------------------------------------|
| 0       | `module-null-sink` becomes default (true silence) | engine stopped; default endpoint muted ("silent mode") |
| 1       | plain `set-default-sink`, no modules        | device made default endpoint, no engine          |
| 2+      | `module-combine-sink` over the set, made default | primary is default + loopback-captured, mirrored to the rest |

**The master row.** While the 2+ fan-out is alive, `list_devices()` prepends a
synthetic device (`DeviceType::Master`, id `sound_multiplexer_combined`, name
"Master volume"). Today only the Linux backend emits it — the Windows fan-out
has no single upstream sink whose volume sits above every device. Its rules:

- It is never a member of an enabled set. `compute_enabled_ids` (lib.rs) and
  `set_all_enabled` (src-tauri/src/lib.rs) both filter it out — it *is* the
  routing, not a routable device.
- Its volume/mute are real (the combine sink's own, upstream of every
  per-device control — and exactly what the system volume keys hit while the
  combine sink is default). On Linux they are explicitly carried across
  combine rebuilds, because a fresh combine sink comes up at 100%.
- It appears only while the fan-out sink is alive: `master_device()` in
  linux.rs requires both a tracked module id and a live sink owned by it, so
  the row vanishes in 0/1-device routing and mid-rebuild.

## Linux backend (`audio/src/linux.rs`)

Everything goes through the `pactl` CLI — no libpulse binding. That keeps the
backend identical across PulseAudio and pipewire-pulse, makes every operation
externally reproducible (the live test drives pactl independently to verify
the backend), and the process-per-call cost is irrelevant at UI interaction
rates. Two version quirks force `LC_ALL=C` on **every** invocation
(`pactl_command()`): `pactl subscribe` output is gettext-localized, so the
monitor's English line matchers (`is_sink_or_server_event`) would go silently
dead on non-English locales; and pactl 16's `-f json` output uses the locale's
decimal separator, which breaks JSON parsing.

**Build-then-switch-then-teardown.** `apply_enabled` orders every transition
so that streams never land on an arbitrary device:

- 2+ devices: load the *replacement* combine module first, then unload the old
  one (the server re-points the default — stored by name — at the same-named
  replacement), then restore master volume/mute by name (unique again only
  after the old sink is gone), then `set-default-sink`, and only then unload a
  leftover null sink. A failed replacement load leaves the current routing and
  tracked state untouched; plain PulseAudio rejects duplicate sink names, so
  there is an unload-then-retry fallback, and if that also fails the backend
  falls back to a single device so `self.enabled` matches reality.
- 1 device: move the default *first*, then unload our modules — never leave
  the system defaulting to a sink about to be destroyed.
- 0 devices: bring up (or reuse) the null sink, move the default onto it, then
  unload the combine.

Skipping any of these orderings drops active streams onto whatever sink the
server picks — an audible failure the user attributes to the app.

**`owner_module` linkage.** pipewire-pulse allows two sinks with the same
name, so a sink name never identifies "our" sink. Every alive-check, the
master row, and startup adoption match sinks to modules via the JSON listing's
`owner_module` field (module ids are unique; `PA_INVALID_INDEX` means none).
`unload_module_checked` only forgets a module id when the module is verifiably
gone — otherwise a zombie sink would survive that the app can no longer
manage.

**Startup adoption.** `reconcile_startup_modules()` handles leftovers from a
crashed run: a leftover combine/null module whose sink still carries the
default is *adopted* (id tracked; the enabled set recovered from the combine's
`slaves=` argument) instead of unloaded — unloading would audibly collapse
routing that may belong to a concurrently running instance. All other
leftovers are unloaded. Sweeping matches the exact sink names
`sound_multiplexer_combined`/`sound_multiplexer_null`, never the prefix, so a
foreign module can never be swept.

**Monitoring.** `start_monitoring` spawns `pactl subscribe` as a child process
and a thread that reads its stdout. Only sink and server events trigger
re-enumeration (sink-input chatter is constant during playback); bursts are
rate-limited to one re-enumeration per 200 ms by *sleeping*, not skipping, so
the last event of a volume drag is never lost. The thread diffs snapshots
(per-sink volume/mute incl. the combine sink, plus the default sink name) and
sends fine-grained or coarse events accordingly. `stop_monitor` kills the
child, which EOFs the pipe and bounds the thread join; `Drop` reaps the child
as a safety net without issuing pactl calls.

**Cleanup guarantees.** `cleanup()` stops the monitor first (so it cannot
react to our own teardown), moves the default onto a real sink (preferring one
from the enabled set) *before* destroying anything, unloads tracked modules,
then sweeps stragglers. Errors are collected, not short-circuited.

## Windows backend (`audio/src/windows/`)

Enumeration, volume and mute use the MMDevice API + `IAudioEndpointVolume`;
change notifications use `IMMNotificationClient` and per-endpoint
`IAudioEndpointVolumeCallback` registrations, reconciled *incrementally*
after every enumeration so they follow hotplug: existing registrations are
kept, never torn down and re-created, because a change landing in an
unregister/re-register window would be silently lost and enumeration runs on
every pump cycle. Every entry point calls `ensure_com()` — a thread-local MTA
join — because calls arrive from arbitrary Tauri threads and COM callbacks
arrive on MTA workers; all WASAPI/MMDevice objects are documented
free-threaded, which is what makes the `Agile` Send wrapper sound. Our own
volume/mute writes carry `APP_EVENT_CONTEXT` so the volume callback can drop
the echo (loop prevention).

**Device classification and Hands-Free filtering.** Real hardware showed the
endpoint's `PKEY_Device_EnumeratorName` is not enough to detect transports:
on audio-offload stacks (Intel SST) a Bluetooth headset's endpoints enumerate
as `INTELAUDIO`, not `BTHENUM`. `TransportInfo` therefore layers the
enumerator name, the adapter interface name, and the undocumented-but-stable
MMDevice bus-path property (`{b3f8fa53-...},39`), whose embedded Bluetooth
service UUID also distinguishes A2DP (`0000110b`) from Hands-Free telephony
(`0000111e`/`0000111f`). HFP endpoints are *excluded* from `list_devices`
and from `apply_enabled`: they coexist with the same headset's A2DP endpoint,
and opening a render stream on one flips the headset out of A2DP — "Select
all" would otherwise collapse the headset to telephony-quality audio and
invalidate its A2DP stream. One row per physical device also matches Linux,
where HFP is a profile, not a separate sink.

**Fan-out engine** (`engine.rs` + platform-neutral `fanout.rs`): the primary
endpoint (kept as Windows default, so it has zero added latency) is
loopback-captured (`AUDCLNT_STREAMFLAGS_LOOPBACK`), and one render thread per
secondary drains a per-device `Ring`. Each render stream pre-fills its device
buffer with 60 ms of silence (`FILL_TARGET_MS`) and then keeps the buffer
topped up *to that level only*, writing just what the ring can supply — the
device buffer itself is the jitter cushion, so a late capture packet lowers
the cushion instead of splicing a silence gap into the stream, and the
secondary lag stays ≈60 ms. Only when the cushion sags below 20 ms
(`FILL_LOW_WATER_MS`) with an empty ring — startup, idle source, or the
secondary's clock running fast — is it topped back up with silence in one
splice. Drift in the other direction (slow secondary) grows the ring until
the 120 ms clamp drops the oldest whole frames back to 60 ms. That is the
whole drift strategy for v1; `IAudioClockAdjustment` rate matching is the
planned refinement. Render streams open with the *capture* mix format plus
`AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, so the OS converts per device and the
engine never touches samples. Silence-flagged capture packets are pushed as
zeros ("silence is data too") so secondaries stay in step instead of
underrunning at random offsets. The capture wait uses a 10 ms timeout because
event-driven loopback is historically unreliable when the endpoint has no
active render stream.

**Failure isolation and self-healing.** A dying render stream takes down only
itself; a capture failure stops the whole engine (`StreamCtx::fatal`). Either
way `fail_stream` raises the `failed` flag and emits `Error` +
`DevicesChanged`. The event pump re-lists devices on that, and
`WindowsBackend::list_devices` runs `reconcile_routing`, which rebuilds the
engine against the surviving devices — at most once per 2 s
(`ENGINE_RESTART_COOLDOWN`), so a persistently failing device cannot drive a
restart loop. A failure landing *inside* the cooldown would consume its only
`DevicesChanged` (a dying stream fires exactly once), so deferral also
schedules a one-shot nudge thread that re-sends `DevicesChanged` just after
the cooldown expires — deferred means retried, never dropped. `reconcile_routing`
also re-anchors a *healthy* fan-out when Windows moves the default endpoint
onto another enabled device (Sound-settings change, hotplug auto-promotion):
the new default becomes the loopback source, so the mirror keeps following
what the system actually plays. A default moved *outside* the enabled set is
the user deliberately routing around the app and is left alone.

**`IPolicyConfig`.** Windows has no documented API to set the default
endpoint. `IPolicyConfig` is the undocumented interface behind the Sound
control panel's own button, stable since Vista and used by every audio
switcher in existence — that ubiquity is the acceptability argument. Only
vtable slot 11 (`set_default_endpoint`) is called; the preceding methods are
declared solely to keep the slot layout correct, pointer params erased.

**Silent mode.** 0 enabled devices mutes the current default endpoint, with
ownership tracking: `silent_muted` stays `None` when the user had already
muted it (that mute is not ours to undo), and a user unmute through the app
takes ownership away and stops re-enforcement. `enforce_silent_mode` is keyed
off present state, not transitions, so failed mutes are retried and the mute
follows the default endpoint if Windows moves it while silent.

**Honest limitations:** secondaries lag the primary by roughly the fill
target (~60 ms) — accepted v1 behavior. Loopback only sees the shared-mode
mix, so exclusive-mode and DRM-protected streams are mirrored as silence.
Loopback also taps the mix *after* the endpoint's software volume and mute,
so on endpoints without hardware volume (typical for Bluetooth and display
audio, and confirmed on real hardware) the primary's volume slider scales —
and its mute silences — every secondary too; a `QueryHardwareSupport`-gated
inverse-gain compensation is the planned refinement. The backend has run on
real hardware: an opt-in live E2E (`audio/tests/windows_live.rs`, below)
passes against real endpoints, including a measured signal-level proof that
the fan-out actually mirrors audio.

## Tauri layer (`src-tauri/src/lib.rs`)

State is `AppState { backend: Mutex<Option<Box<dyn AudioBackend>>>, monitor_tx }`.
The backend is `None` when the audio server was unreachable at startup;
`refresh_devices` retries creation and re-arms monitoring, so the user can fix
the server and hit Refresh instead of restarting the app.

Commands are `async` so they run on the Tauri async runtime rather than the
UI thread — every backend call may shell out (pactl) or block on the mutex
behind the event pump. Each command body lives in a plain `*_inner` function
taking the state pieces directly, which is what the unit tests drive against
a `MockBackend`.

**Single-emitter event architecture.** Authoritative device state reaches the
UI on exactly one path: the `devices-changed` events emitted by the
`event_pump` thread, which drains the monitor channel, coalesces bursts
(80 ms sleep + `try_recv` drain — one sink change yields several subscribe
lines), re-lists devices, and emits full state. Mutating commands return `()`
and instead `notify()` the pump by sending a synthetic `DevicesChanged` — even
on failure. The race this prevents: if commands returned device lists, a slow
command response could arrive *after* a fresher monitor-driven event and
overwrite it in the UI; with one emitting thread, emissions are totally
ordered. Only `get_devices`/`refresh_devices` return lists (initial load and
explicit user refresh). `set_device_volume` deliberately does not notify —
during drags the Linux monitor (or the optimistic UI on Windows, which
suppresses its own echo) keeps the UI truthful, and full snapshots at drag
rate would only churn.

Errors surface on two paths: command rejections (`Result<T, String>`,
`anyhow` chains formatted with `{:#}`) and the `backend-error` event, fed by
`BackendEvent::Error` and by list failures inside the pump. `cleanup()` runs
on `RunEvent::Exit`. `main.rs` sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` on
Linux (unless the user set it) — WebKitGTK's DMA-BUF renderer kills the
Wayland connection on some driver/compositor combinations.

## Frontend (`ui/`)

Vanilla JS (`main.js`, one IIFE) over `index.html` + `styles.css`. No
framework: the UI is one list with three controls per row; the entire state
model is "replace `devices`, re-render", and Tauri's `withGlobalTauri` global
plus a `<template>` element cover the rest. This keeps the frontend
dependency-free and directly loadable in jsdom for tests.

- **Keyed in-place rendering.** `render()` keeps a `Map` of device id → `<li>`
  and updates rows in place; order reconciliation walks a cursor and calls
  `insertBefore` only on misplaced rows. Re-appending an already-placed node
  would blur focus and kill an in-progress slider drag — `draggingId`
  additionally guards the dragged slider's value against incoming snapshots.
- **Optimistic updates + revert.** Toggles/mutes update the DOM before the
  invoke resolves; on rejection `revertOnError` flashes the error and refetches
  authoritative state, so the UI never keeps showing routing that isn't real.
  The error flash owns the status bar for 5 s (the revert repaint must not
  paint over it before the user can read it).
- **Throttling.** Volume drags push at most every 80 ms (trailing-edge
  throttle); the final `change` event sends the exact end value.
- **Mock mode.** Without `window.__TAURI__` an in-memory mock backend
  activates, so the UI can be previewed in a plain browser; the DOM tests
  drive either this mock or a fake `__TAURI__` object.
- **Theming.** One token set in `styles.css` (`:root` custom properties,
  oklch). Dark applies via `prefers-color-scheme` (`:root:not([data-theme="light"])`)
  and via an explicit `:root[data-theme="dark"]` override; the System/Light/
  Dark choice persists in `localStorage`. Master-row rules sit at the end of
  the file: `[data-master="true"]` hides the enable switch — the switch exists
  in the DOM, CSS is the hiding contract.

## Testing

| Suite | Run | Covers |
|-------|-----|--------|
| Rust unit tests | `cargo test --workspace` | pactl parsers, device-type heuristics, module matching, ring buffer/format math (`fanout.rs`, OS-free so it runs everywhere), Windows type inference, command logic against a `MockBackend` (notify-exactly-once, clamping, failure paths) |
| Live smoke | `cargo test -p sound-multiplexer-audio -- --ignored` | `linux.rs::live_smoke`: loads/unloads a combine sink against the real server; never touches the default or any volume |
| Live E2E | `cargo test -p sound-multiplexer-audio --test linux_live -- --ignored --nocapture` | full `LinuxBackend` lifecycle (below) |
| Live E2E (Windows) | `cargo test -p sound-multiplexer-audio --test windows_live -- --ignored --nocapture` | full `WindowsBackend` lifecycle on real endpoints (below) |
| Frontend | `cd ui-tests && npm install && npm test` | the real `index.html` + `main.js` in jsdom: rendering, keyed reconciliation (identity + minimal moves), interactions, exact IPC payloads, revert-on-error, master row, theming |

The live E2E (`audio/tests/linux_live.rs`) exercises every routing transition
(1 → 2+ → rebuild → 0 → 1), idempotent re-apply (same combine module id),
master-row placement, master volume set *and preserved across a forced combine
rebuild*, volume/mute on a non-enabled device, monitoring incl. restart, and
zero-leftover cleanup — asserting through independent pactl calls, not the
backend's own plumbing. Its safety guarantees: a **skip-gate** (it refuses to
run when the server is unreachable, any `sound_multiplexer` module or sink
already exists — a live app instance may own it — or there is no sink/default),
and an `AudioStateGuard` whose `Drop` restores the default sink, every sink's
per-channel volumes and mute, and sweeps test modules even when an assertion
panics mid-test. Machines with fewer than two sinks get helper null sinks
named outside the app's prefix.

The Windows live E2E (`audio/tests/windows_live.rs`) mirrors the Linux one
with independent plumbing (its own enumerator, endpoint-volume reads and
`IPolicyConfig` declaration): device listing sanity, external-change
monitoring, volume/mute writes, 1-device default switching (bogus ids
ignored), 2+ fan-out, idempotent re-apply, silent mode in and out, and
cleanup. Its fan-out check needs no listener: it plays a sine tone on the
primary (the system default, exactly like any app) and loopback-captures a
*secondary*, asserting signal energy that can only have arrived through the
engine. An `AudioStateGuard` restores the default endpoint and every
endpoint's volume/mute on `Drop`, even when an assertion panics. It requires
real endpoints, so it is opt-in on real hardware, not part of CI.

CI (`.github/workflows/ci.yml`) runs all of the above except the live E2Es.
The Linux job adds its live E2E inside a `dbus-run-session` with a real
PipeWire/pipewire-pulse/ WirePlumber stack and two **synthetic null-sink
devices** (`ci_dev_a`/`ci_dev_b`, named outside the app's prefix so the
backend treats them as ordinary outputs), plus clippy for the host and a
cross-check clippy for `x86_64-pc-windows-msvc`. The Windows job runs
`cargo test --workspace` on `windows-latest` (no audio devices there — the
live E2E needs real hardware) and builds an unsigned NSIS installer artifact.
A third job uploads deb/rpm/AppImage bundles.

## Roadmap

Packaging (decisions researched, not yet executed): Linux ships via Flathub
plus AppImage (CI already produces AppImage/deb/rpm artifacts); Windows via
the NSIS installer (CI builds it unsigned today), then winget and the
Microsoft Store. macOS comes later through a third `AudioBackend`
implementation on CoreAudio **aggregate devices** — the platform's native
equivalent of `module-combine-sink` — which fits the existing trait without
changes to the Tauri layer or UI.
