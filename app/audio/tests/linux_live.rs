//! Full end-to-end test of `LinuxBackend` against the REAL PipeWire (or
//! PulseAudio) server of this machine.
//!
//! Run with:
//!   cargo test -p sound-multiplexer-audio --test linux_live -- --ignored --nocapture
//!
//! The test mutates live routing — default sink, our combine/null modules,
//! sink volumes and mutes — and restores every bit of it through a Drop
//! guard, so the machine ends up exactly as found even when an assertion
//! panics mid-test. If any sound_multiplexer module is already loaded (a
//! running app instance may own it) the test SKIPS instead of fighting over
//! the routing. When fewer than two real sinks exist, a helper
//! `module-null-sink` (named OUTSIDE the app's prefix, so the backend treats
//! it as a real device) provides the second device the combine path needs.

#![cfg(target_os = "linux")]

use sound_multiplexer_audio::linux::LinuxBackend;
use sound_multiplexer_audio::{AudioBackend, BackendEvent};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const OUR_PREFIX: &str = "sound_multiplexer";
const COMBINED_SINK: &str = "sound_multiplexer_combined";
const NULL_SINK: &str = "sound_multiplexer_null";
/// Helper second device; deliberately NOT under the app's prefix.
const AUX_SINK: &str = "smx_livetest_aux";

/// PipeWire applies routing asynchronously; poll this long after a change.
const APPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// Budget for a `pactl subscribe` event to reach the monitor channel.
const EVENT_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// pactl helpers (the backend's own plumbing is private, so the test drives
// pactl independently — which also makes the assertions genuinely external)
// ---------------------------------------------------------------------------

fn run_pactl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("pactl")
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .map_err(|e| format!("could not run pactl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`pactl {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn pactl(args: &[&str]) -> String {
    run_pactl(args).unwrap_or_else(|e| panic!("{e}"))
}

/// Best-effort pactl for restoration paths: must never panic inside Drop.
fn pactl_lenient(args: &[&str]) {
    if let Err(e) = run_pactl(args) {
        eprintln!("restore: {e}");
    }
}

fn default_sink() -> String {
    pactl(&["get-default-sink"]).trim().to_string()
}

fn sink_names() -> Vec<String> {
    pactl(&["list", "short", "sinks"])
        .lines()
        .filter_map(|line| Some(line.split('\t').nth(1)?.trim().to_string()))
        .collect()
}

/// Rows of `pactl list short modules` whose arguments mention
/// "sound_multiplexer" at all — deliberately broader than the backend's
/// exact-sink-name matching, so the safety gate and the zero-leftovers
/// assertions cannot miss anything of ours.
fn our_module_rows(listing: &str) -> Vec<(u32, String)> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let id: u32 = fields.next()?.trim().parse().ok()?;
            let _module_name = fields.next()?;
            let args = fields.next().unwrap_or("").trim();
            args.contains(OUR_PREFIX).then(|| (id, args.to_string()))
        })
        .collect()
}

fn arg_value<'a>(args: &'a str, key: &str) -> Option<&'a str> {
    args.split_whitespace()
        .find_map(|token| token.strip_prefix(key)?.strip_prefix('='))
}

/// (module id, slave sink names) of our combine module, when loaded.
fn combine_module(listing: &str) -> Option<(u32, Vec<String>)> {
    our_module_rows(listing).into_iter().find_map(|(id, args)| {
        (arg_value(&args, "sink_name") == Some(COMBINED_SINK)).then(|| {
            let slaves = arg_value(&args, "slaves")
                .map(|v| v.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            (id, slaves)
        })
    })
}

fn null_module_loaded(listing: &str) -> bool {
    our_module_rows(listing)
        .iter()
        .any(|(_, args)| arg_value(args, "sink_name") == Some(NULL_SINK))
}

fn find_json_sink(name: &str) -> Option<serde_json::Value> {
    let sinks: serde_json::Value =
        serde_json::from_str(&pactl(&["-f", "json", "list", "sinks"]))
            .expect("unparseable `pactl -f json list sinks` output");
    sinks
        .as_array()
        .expect("sink listing is not a JSON array")
        .iter()
        .find(|s| s["name"] == name)
        .cloned()
}

/// Per-channel `value_percent` strings of a sink, from the JSON listing.
fn json_channel_percents(sink: &serde_json::Value) -> Vec<String> {
    sink["volume"]
        .as_object()
        .map(|channels| {
            channels
                .values()
                .filter_map(|c| Some(c["value_percent"].as_str()?.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Raw per-channel volumes in channel-map order, parsed from the text
/// output of `pactl get-sink-volume` (the JSON listing sorts channels
/// alphabetically, which would break exact multi-channel restoration).
fn sink_channel_volumes(name: &str) -> Vec<u64> {
    let out = pactl(&["get-sink-volume", name]);
    let line = out.lines().next().unwrap_or("");
    let line = line.trim_start().strip_prefix("Volume:").unwrap_or(line);
    // "front-left: 39649 /  61% / -13.06 dB,   front-right: ..."
    line.split(',')
        .filter_map(|channel| {
            let raw = channel.split('/').next()?;
            raw.rsplit(':').next()?.trim().parse().ok()
        })
        .collect()
}

fn sink_muted(name: &str) -> bool {
    pactl(&["get-sink-mute", name]).contains("yes")
}

// ---------------------------------------------------------------------------
// State restoration guard
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct SinkState {
    name: String,
    /// Raw per-channel volumes, in channel-map order.
    channel_volumes: Vec<u64>,
    muted: bool,
}

/// Captures default sink + per-sink volume/mute at construction and restores
/// all of it (plus sweeping any module the test left behind) from Drop, so
/// restoration also happens when an assertion panics mid-test.
struct AudioStateGuard {
    default_sink: String,
    sinks: Vec<SinkState>,
    /// Helper second-device module to unload, if the test loaded one.
    aux_module: Option<u32>,
}

impl AudioStateGuard {
    fn capture() -> Self {
        AudioStateGuard {
            default_sink: default_sink(),
            sinks: sink_names()
                .into_iter()
                .map(|name| SinkState {
                    channel_volumes: sink_channel_volumes(&name),
                    muted: sink_muted(&name),
                    name,
                })
                .collect(),
            aux_module: None,
        }
    }
}

impl Drop for AudioStateGuard {
    fn drop(&mut self) {
        eprintln!("restoring captured audio state");
        // Default first: it is a real sink, so nothing we unload next
        // carries the default while being destroyed.
        pactl_lenient(&["set-default-sink", &self.default_sink]);
        if let Ok(listing) = run_pactl(&["list", "short", "modules"]) {
            for (id, args) in our_module_rows(&listing) {
                eprintln!("restore: unloading leftover module #{id} ({args})");
                pactl_lenient(&["unload-module", &id.to_string()]);
            }
        }
        if let Some(id) = self.aux_module {
            pactl_lenient(&["unload-module", &id.to_string()]);
        }
        for sink in &self.sinks {
            if !sink.channel_volumes.is_empty() {
                let volumes: Vec<String> =
                    sink.channel_volumes.iter().map(u64::to_string).collect();
                let mut args = vec!["set-sink-volume", sink.name.as_str()];
                args.extend(volumes.iter().map(String::as_str));
                pactl_lenient(&args);
            }
            pactl_lenient(&["set-sink-mute", &sink.name, if sink.muted { "1" } else { "0" }]);
        }
    }
}

// ---------------------------------------------------------------------------
// Waiting helpers
// ---------------------------------------------------------------------------

/// Poll `probe` every 50ms until it yields a value; panic after `timeout`.
fn wait_for<T>(what: &str, timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {timeout:?} waiting for {what}"
        );
        std::thread::sleep(POLL);
    }
}

/// Drain monitor events until one matches `wanted`; panic after EVENT_TIMEOUT.
fn wait_for_event(
    rx: &mpsc::Receiver<BackendEvent>,
    what: &str,
    mut wanted: impl FnMut(&BackendEvent) -> bool,
) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(event) if wanted(&event) => return,
            Ok(other) => eprintln!("  (ignoring interim event: {other:?})"),
            Err(e) => panic!("no {what} event within {EVENT_TIMEOUT:?}: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[test]
#[ignore = "mutates the live audio server"]
fn full_lifecycle_against_live_server() {
    // --- Safety gate: never touch a server another instance is driving ---
    if let Err(e) = run_pactl(&["info"]) {
        eprintln!("SKIP: no reachable sound server: {e}");
        return;
    }
    let leftovers = our_module_rows(&pactl(&["list", "short", "modules"]));
    if !leftovers.is_empty() {
        eprintln!(
            "SKIP: sound_multiplexer modules already loaded (a live app \
             instance may own them): {leftovers:?}"
        );
        return;
    }
    if sink_names().iter().any(|n| n.starts_with(OUR_PREFIX)) {
        eprintln!("SKIP: a sound_multiplexer sink already exists");
        return;
    }
    let real_sinks: Vec<String> = sink_names()
        .into_iter()
        .filter(|n| !n.starts_with(OUR_PREFIX))
        .collect();
    if real_sinks.is_empty() {
        eprintln!("SKIP: no output devices on this machine");
        return;
    }
    if default_sink().is_empty() {
        eprintln!("SKIP: server reports no default sink");
        return;
    }

    // --- Capture state; the guard restores it even on panic --------------
    let mut guard = AudioStateGuard::capture();
    let original_default = guard.default_sink.clone();
    let original_sinks = guard.sinks.clone();
    eprintln!("captured state: default '{original_default}', {} sinks", original_sinks.len());

    // --- Provision a second device if the machine has only one sink ------
    if real_sinks.len() < 2 {
        eprintln!("one real sink only; loading helper sink '{AUX_SINK}' as the second device");
        let sink_name_arg = format!("sink_name={AUX_SINK}");
        let id: u32 = pactl(&[
            "load-module",
            "module-null-sink",
            sink_name_arg.as_str(),
            "sink_properties=device.description='Sound-Multiplexer-Livetest-Aux'",
        ])
        .trim()
        .parse()
        .expect("`pactl load-module` did not print a module id");
        guard.aux_module = Some(id);
        wait_for("helper sink to appear", APPLY_TIMEOUT, || {
            sink_names().iter().any(|n| n == AUX_SINK).then_some(())
        });
    }

    // --- Backend construction and enumeration ----------------------------
    let mut backend = LinuxBackend::new().expect("backend construction failed");
    let devices = backend.list_devices().expect("list_devices failed");
    assert!(!devices.is_empty(), "no devices enumerated");
    let live_names = sink_names();
    for device in &devices {
        assert!(
            !device.id.starts_with(OUR_PREFIX),
            "our sink leaked into the device list: {}",
            device.id
        );
        assert!(
            live_names.contains(&device.id),
            "device id '{}' is not a live sink name",
            device.id
        );
    }
    assert!(devices.len() >= 2, "need two devices for the combine path");

    // The single-device target is a NON-default device, so "the default
    // moved" is an observable transition, not a pre-existing state.
    let start_default = default_sink();
    let sink_a = devices
        .iter()
        .find(|d| d.id != start_default)
        .unwrap_or(&devices[0])
        .id
        .clone();
    let sink_b = devices
        .iter()
        .find(|d| d.id != sink_a)
        .expect("second device vanished")
        .id
        .clone();
    eprintln!("devices under test: '{sink_a}' and '{sink_b}'");

    // --- 1 enabled: plain default, no modules -----------------------------
    backend.apply_enabled(std::slice::from_ref(&sink_a)).expect("apply_enabled([a]) failed");
    wait_for("default sink to move to the single enabled device", APPLY_TIMEOUT, || {
        (default_sink() == sink_a).then_some(())
    });
    assert!(
        our_module_rows(&pactl(&["list", "short", "modules"])).is_empty(),
        "single-device routing must not load any module"
    );
    eprintln!("single device routing OK");

    // --- 2 enabled: combine sink over both, and it is the default --------
    backend
        .apply_enabled(&[sink_a.clone(), sink_b.clone()])
        .expect("apply_enabled([a, b]) failed");
    let (combine_id, slaves) = wait_for(
        "combine sink to be loaded and become default",
        APPLY_TIMEOUT,
        || {
            let found = combine_module(&pactl(&["list", "short", "modules"]))?;
            (default_sink() == COMBINED_SINK).then_some(found)
        },
    );
    let mut sorted_slaves = slaves.clone();
    sorted_slaves.sort();
    let mut expected = vec![sink_a.clone(), sink_b.clone()];
    expected.sort();
    assert_eq!(sorted_slaves, expected, "combine sink must slave BOTH devices");
    // The combined sink itself never shows up as a device, and `enabled`
    // reflects exactly the applied set.
    let devices_now = backend.list_devices().expect("list_devices failed");
    assert!(devices_now.iter().all(|d| !d.id.starts_with(OUR_PREFIX)));
    for device in &devices_now {
        assert_eq!(
            device.enabled,
            device.id == sink_a || device.id == sink_b,
            "wrong enabled flag for '{}'",
            device.id
        );
    }
    eprintln!("combined routing OK (module #{combine_id}, slaves {slaves:?})");

    // --- Idempotency: same set must not rebuild the combine sink ----------
    // (wait for the sink to be enumerable first: the short-circuit check
    // verifies module liveness through the JSON sink listing)
    wait_for("combined sink to appear in the sink listing", APPLY_TIMEOUT, || {
        let sink = find_json_sink(COMBINED_SINK)?;
        (sink["owner_module"].as_u64() == Some(u64::from(combine_id))).then_some(())
    });
    backend
        .apply_enabled(&[sink_a.clone(), sink_b.clone()])
        .expect("idempotent apply_enabled failed");
    let (id_after, _) = combine_module(&pactl(&["list", "short", "modules"]))
        .expect("combine sink vanished after idempotent re-apply");
    assert_eq!(
        id_after, combine_id,
        "re-applying the same set must keep the combine module (no audible rebuild)"
    );
    eprintln!("idempotent re-apply OK");

    // --- 0 enabled: null sink takes over -----------------------------------
    backend.apply_enabled(&[]).expect("apply_enabled([]) failed");
    wait_for("null sink to become default", APPLY_TIMEOUT, || {
        (default_sink() == NULL_SINK).then_some(())
    });
    wait_for("combine module to be unloaded", APPLY_TIMEOUT, || {
        combine_module(&pactl(&["list", "short", "modules"]))
            .is_none()
            .then_some(())
    });
    eprintln!("silence routing OK");

    // --- Back to 1: null sink retired, device is default -------------------
    backend.apply_enabled(std::slice::from_ref(&sink_a)).expect("apply_enabled([a]) failed");
    wait_for("default sink to return to the single device", APPLY_TIMEOUT, || {
        (default_sink() == sink_a).then_some(())
    });
    wait_for("null module to be unloaded", APPLY_TIMEOUT, || {
        (!null_module_loaded(&pactl(&["list", "short", "modules"]))).then_some(())
    });
    eprintln!("silence -> single device OK");

    // --- Volume / mute land on the physical sink ---------------------------
    backend.set_volume(&sink_b, 0.37).expect("set_volume failed");
    wait_for("volume 37% to reach the sink", APPLY_TIMEOUT, || {
        let percents = json_channel_percents(&find_json_sink(&sink_b)?);
        (!percents.is_empty() && percents.iter().all(|p| p == "37%")).then_some(())
    });
    backend.set_muted(&sink_b, true).expect("set_muted(true) failed");
    wait_for("mute to reach the sink", APPLY_TIMEOUT, || {
        (find_json_sink(&sink_b)?["mute"] == true).then_some(())
    });
    backend.set_muted(&sink_b, false).expect("set_muted(false) failed");
    wait_for("unmute to reach the sink", APPLY_TIMEOUT, || {
        (find_json_sink(&sink_b)?["mute"] == false).then_some(())
    });
    eprintln!("volume/mute round-trip OK");

    // --- Monitoring: external changes surface as events --------------------
    let (tx, rx) = mpsc::channel();
    backend.start_monitoring(tx).expect("start_monitoring failed");
    // The monitor takes its baseline snapshot on the fresh thread; give it
    // (and the pactl subscription) a moment so our change lands afterwards.
    std::thread::sleep(Duration::from_millis(500));
    pactl(&["set-sink-volume", sink_b.as_str(), "45%"]);
    wait_for_event(&rx, "VolumeChanged(45%)", |event| {
        matches!(event, BackendEvent::VolumeChanged { id, volume }
            if *id == sink_b && (*volume - 0.45).abs() < 0.02)
    });
    eprintln!("monitoring OK");

    // --- Monitoring is restart-safe (sound-server-restart recovery path) ---
    let (tx2, rx2) = mpsc::channel();
    backend.start_monitoring(tx2).expect("second start_monitoring failed");
    std::thread::sleep(Duration::from_millis(500));
    pactl(&["set-sink-volume", sink_b.as_str(), "60%"]);
    wait_for_event(&rx2, "VolumeChanged(60%) after monitor restart", |event| {
        matches!(event, BackendEvent::VolumeChanged { id, volume }
            if *id == sink_b && (*volume - 0.60).abs() < 0.02)
    });
    drop(rx);
    eprintln!("monitor restart OK");

    // --- cleanup(): zero leftovers, default on a real sink -----------------
    backend.cleanup().expect("cleanup failed");
    assert!(
        our_module_rows(&pactl(&["list", "short", "modules"])).is_empty(),
        "cleanup left sound_multiplexer modules behind"
    );
    let after_cleanup = default_sink();
    assert!(
        !after_cleanup.starts_with(OUR_PREFIX),
        "default still on one of our sinks after cleanup: {after_cleanup}"
    );
    assert!(
        sink_names().contains(&after_cleanup),
        "default '{after_cleanup}' is not a live sink"
    );
    drop(backend);
    eprintln!("cleanup OK");

    // --- Restore and verify the machine is exactly as found ----------------
    drop(guard);
    wait_for("original default sink to be restored", APPLY_TIMEOUT, || {
        (default_sink() == original_default).then_some(())
    });
    for sink in &original_sinks {
        wait_for(
            &format!("volume/mute of '{}' to be restored", sink.name),
            APPLY_TIMEOUT,
            || {
                (sink_channel_volumes(&sink.name) == sink.channel_volumes
                    && sink_muted(&sink.name) == sink.muted)
                    .then_some(())
            },
        );
    }
    assert!(
        our_module_rows(&pactl(&["list", "short", "modules"])).is_empty(),
        "sound_multiplexer modules survived restoration"
    );
    assert!(
        !sink_names().iter().any(|n| n == AUX_SINK),
        "helper sink survived restoration"
    );
    eprintln!("state restored: default and all sink volumes/mutes as captured");
}
