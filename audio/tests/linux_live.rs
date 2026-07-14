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
//! it as a real device) provides the second device the combine path needs;
//! a second helper sink is always loaded mid-test to force a combine REBUILD
//! for the master-volume preservation check.

#![cfg(target_os = "linux")]

use sound_multiplexer_audio::linux::LinuxBackend;
use sound_multiplexer_audio::{AudioBackend, BackendEvent, DeviceType};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const OUR_PREFIX: &str = "sound_multiplexer";
const COMBINED_SINK: &str = "sound_multiplexer_combined";
const NULL_SINK: &str = "sound_multiplexer_null";
/// Helper second device; deliberately NOT under the app's prefix.
const AUX_SINK: &str = "smx_livetest_aux";
/// Helper third device, loaded mid-test to force a combine-sink REBUILD
/// (a genuinely different 2+ set) for the master-volume preservation check.
const AUX2_SINK: &str = "smx_livetest_aux2";

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

/// Like [`find_json_sink`], but matched by owning module id: during a combine
/// rebuild two same-named sinks can briefly coexist (pipewire-pulse allows
/// it), and only the module id disambiguates the replacement from the corpse.
fn find_json_sink_by_owner(module_id: u32) -> Option<serde_json::Value> {
    let sinks: serde_json::Value =
        serde_json::from_str(&pactl(&["-f", "json", "list", "sinks"]))
            .expect("unparseable `pactl -f json list sinks` output");
    sinks
        .as_array()
        .expect("sink listing is not a JSON array")
        .iter()
        .find(|s| s["owner_module"].as_u64() == Some(u64::from(module_id)))
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
    /// Helper-device modules (aux/aux2) to unload, if the test loaded any.
    aux_modules: Vec<u32>,
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
            aux_modules: Vec::new(),
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
        for id in &self.aux_modules {
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

/// Load a helper `module-null-sink` named OUTSIDE the app's prefix (the
/// backend treats it as a real device) and wait until it is enumerable.
/// Returns the module id; the caller must hand it to the guard.
fn load_helper_sink(name: &str) -> u32 {
    let sink_name_arg = format!("sink_name={name}");
    let id: u32 = pactl(&[
        "load-module",
        "module-null-sink",
        sink_name_arg.as_str(),
        "sink_properties=device.description='Sound-Multiplexer-Livetest-Aux'",
    ])
    .trim()
    .parse()
    .expect("`pactl load-module` did not print a module id");
    wait_for("helper sink to appear", APPLY_TIMEOUT, || {
        sink_names().iter().any(|n| n == name).then_some(())
    });
    id
}

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
        guard.aux_modules.push(load_helper_sink(AUX_SINK));
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
    // No combine sink alive -> no synthetic master row.
    assert!(
        backend
            .list_devices()
            .expect("list_devices failed")
            .iter()
            .all(|d| d.device_type != DeviceType::Master),
        "single-device routing must not surface a master row"
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
    // Wait for the sink to be enumerable: the master row — and the
    // alive-checks in apply_enabled — go through the JSON sink listing.
    wait_for("combined sink to appear in the sink listing", APPLY_TIMEOUT, || {
        let sink = find_json_sink(COMBINED_SINK)?;
        (sink["owner_module"].as_u64() == Some(u64::from(combine_id))).then_some(())
    });
    // With the combine alive, the synthetic master row LEADS the device
    // list; the real rows' `enabled` reflects exactly the applied set.
    let devices_now = backend.list_devices().expect("list_devices failed");
    let master = devices_now
        .first()
        .expect("device list empty while the combine sink is alive");
    assert_eq!(master.id, COMBINED_SINK, "master row must lead the device list");
    assert_eq!(master.device_type, DeviceType::Master);
    assert_eq!(master.name, "Master volume");
    assert!(master.enabled, "the master row is always enabled");
    assert!(
        devices_now[1..].iter().all(|d| !d.id.starts_with(OUR_PREFIX)),
        "our sinks leaked into the real-device rows"
    );
    assert!(
        devices_now[1..].iter().all(|d| d.device_type != DeviceType::Master),
        "only the leading row may be the master"
    );
    for device in &devices_now[1..] {
        assert_eq!(
            device.enabled,
            device.id == sink_a || device.id == sink_b,
            "wrong enabled flag for '{}'",
            device.id
        );
    }
    eprintln!("combined routing OK (module #{combine_id}, slaves {slaves:?})");

    // --- Idempotency: same set must not rebuild the combine sink ----------
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

    // --- Master volume: settable via the ordinary set_volume ---------------
    backend
        .set_volume(COMBINED_SINK, 0.4)
        .expect("set_volume on the master row failed");
    wait_for("master volume 40% to reach the combine sink", APPLY_TIMEOUT, || {
        let percents = json_channel_percents(&find_json_sink(COMBINED_SINK)?);
        (!percents.is_empty() && percents.iter().all(|p| p == "40%")).then_some(())
    });
    let master_row = backend
        .list_devices()
        .expect("list_devices failed")
        .into_iter()
        .find(|d| d.device_type == DeviceType::Master)
        .expect("master row vanished after set_volume");
    assert!(
        (master_row.volume - 0.40).abs() < 0.02,
        "master row must report the combine sink's real volume, got {}",
        master_row.volume
    );
    eprintln!("master volume set OK");

    // --- Master volume survives a combine REBUILD --------------------------
    // Growing the enabled set to a genuinely different 2+ composition tears
    // the combine module down and replaces it. A fresh combine sink comes up
    // at 100%, so the 40% master volume must be carried over explicitly.
    eprintln!("loading helper sink '{AUX2_SINK}' to force a combine rebuild");
    guard.aux_modules.push(load_helper_sink(AUX2_SINK));
    backend
        .apply_enabled(&[sink_a.clone(), sink_b.clone(), AUX2_SINK.to_string()])
        .expect("apply_enabled([a, b, aux2]) failed");
    let (rebuilt_id, rebuilt_slaves) = combine_module(&pactl(&["list", "short", "modules"]))
        .expect("combine module gone after the rebuild");
    assert_ne!(
        rebuilt_id, combine_id,
        "a different enabled set must REBUILD the combine module"
    );
    assert_eq!(
        rebuilt_slaves.len(),
        3,
        "rebuilt combine must slave all three devices: {rebuilt_slaves:?}"
    );
    wait_for(
        "master volume to be preserved at 40% across the rebuild",
        APPLY_TIMEOUT,
        || {
            let percents = json_channel_percents(&find_json_sink_by_owner(rebuilt_id)?);
            (!percents.is_empty() && percents.iter().all(|p| p == "40%")).then_some(())
        },
    );
    let master_row = backend
        .list_devices()
        .expect("list_devices failed")
        .into_iter()
        .find(|d| d.device_type == DeviceType::Master)
        .expect("master row vanished after the rebuild");
    assert!(
        (master_row.volume - 0.40).abs() < 0.02,
        "the rebuild reset the master volume, got {}",
        master_row.volume
    );
    eprintln!(
        "master volume preserved across rebuild OK (module #{combine_id} -> #{rebuilt_id})"
    );

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
    // The combine sink is gone -> the master row is gone with it.
    assert!(
        backend
            .list_devices()
            .expect("list_devices failed")
            .iter()
            .all(|d| d.device_type != DeviceType::Master),
        "silence routing must not surface a master row"
    );
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
    // Only sink_a is enabled here, so sink_b is a NON-enabled device: its
    // volume must be adjustable anyway, changing the stored level without
    // altering the enabled set or the routing.
    backend.set_volume(&sink_b, 0.37).expect("set_volume failed");
    wait_for("volume 37% to reach the sink", APPLY_TIMEOUT, || {
        let percents = json_channel_percents(&find_json_sink(&sink_b)?);
        (!percents.is_empty() && percents.iter().all(|p| p == "37%")).then_some(())
    });
    assert_eq!(
        default_sink(),
        sink_a,
        "volume on a non-enabled device must not move the default"
    );
    assert!(
        our_module_rows(&pactl(&["list", "short", "modules"])).is_empty(),
        "volume on a non-enabled device must not load any module"
    );
    let devices_after = backend.list_devices().expect("list_devices failed");
    assert!(
        devices_after.iter().all(|d| d.enabled == (d.id == sink_a)),
        "volume on a non-enabled device must not alter the enabled set"
    );
    let b_row = devices_after
        .iter()
        .find(|d| d.id == sink_b)
        .expect("sink_b missing from the device list");
    assert!(
        (b_row.volume - 0.37).abs() < 0.02,
        "sink_b must report the freshly set volume, got {}",
        b_row.volume
    );
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
        !sink_names().iter().any(|n| n == AUX_SINK || n == AUX2_SINK),
        "helper sink survived restoration"
    );
    eprintln!("state restored: default and all sink volumes/mutes as captured");
}
