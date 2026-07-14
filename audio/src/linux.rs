//! Linux backend: drives PulseAudio (or PipeWire via pipewire-pulse)
//! through the `pactl` CLI.
//!
//! Routing:
//!   2+ enabled -> `pactl load-module module-combine-sink
//!                  sink_name=sound_multiplexer_combined slaves=a,b,...`
//!                 then `pactl set-default-sink sound_multiplexer_combined`
//!                 (the replacement loads before any old sink of ours is
//!                 unloaded, so streams never fall back to a random device)
//!   1 enabled  -> no module; `pactl set-default-sink <sink>` (default moves
//!                 first so streams migrate before our old sink is destroyed)
//!   0 enabled  -> `module-null-sink sink_name=sound_multiplexer_null` as
//!                 default (reused if already loaded)
//!
//! Enumeration and volume/mute state come from `pactl -f json list sinks`.
//! Module IDs are captured from `load-module` stdout and reconciled against
//! `pactl list short modules` (tab-separated: id, name, args), matching our
//! exact sink names only. At startup, a leftover module of ours that still
//! carries the default sink is adopted (crash recovery without an audible
//! drop; a concurrent instance's live routing is never torn down) and all
//! other leftovers are unloaded; cleanup() sweeps every module of ours.
//! Every pactl invocation runs under LC_ALL=C: `pactl subscribe` output
//! (parsed by the monitor thread) is gettext-localized, and pactl 16's JSON
//! uses the locale's decimal separator.
//! Monitoring runs `pactl subscribe` as a child process on a thread.

use crate::{AudioBackend, BackendEvent, Device, DeviceType};
use anyhow::{bail, Context};
use log::{debug, info, warn};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Prefix of every sink this app creates; enumeration skips such sinks.
/// (Module sweeping matches the exact sink names below, never the prefix,
/// so it can never unload a foreign module.)
const OUR_PREFIX: &str = "sound_multiplexer";
const COMBINED_SINK: &str = "sound_multiplexer_combined";
const NULL_SINK: &str = "sound_multiplexer_null";

/// `owner_module` value PulseAudio uses for "no owning module".
const PA_INVALID_INDEX: u64 = 4_294_967_295;

/// Minimum spacing between full re-enumerations in the monitor thread.
const REENUM_MIN_INTERVAL: Duration = Duration::from_millis(200);

/// PulseAudio/PipeWire backend; see the module docs for the routing scheme
/// and pactl conventions.
pub struct LinuxBackend {
    /// Sink names the user enabled, as last applied.
    enabled: HashSet<String>,
    /// Module id of our combine sink, if loaded.
    combine_module: Option<u32>,
    /// Module id of our null (silence) sink, if loaded.
    null_module: Option<u32>,
    monitor_child: Option<Child>,
    monitor_thread: Option<JoinHandle<()>>,
}

impl LinuxBackend {
    /// Fails fast when the sound server is unreachable. Reconciles leftover
    /// modules of previous runs, then seeds the enabled set from the current
    /// default sink.
    pub fn new() -> anyhow::Result<Self> {
        run_pactl(&["info"]).context("cannot reach the PulseAudio/PipeWire sound server")?;

        let mut backend = LinuxBackend {
            enabled: HashSet::new(),
            combine_module: None,
            null_module: None,
            monitor_child: None,
            monitor_thread: None,
        };

        // Reconcile combine/null modules left behind by a previous run
        // before reading the default, so an orphaned leftover never becomes
        // "enabled". A leftover that still carries the default is adopted
        // rather than unloaded.
        backend.reconcile_startup_modules()?;

        let default = get_default_sink()?;
        if !default.is_empty() && !is_ours(&default) {
            let sinks = list_sinks()?;
            if sinks.iter().any(|s| s.name == default) {
                debug!("startup: default sink '{default}' is the initially enabled device");
                backend.enabled.insert(default);
            }
        }
        Ok(backend)
    }

    /// Startup reconciliation of modules left over from previous runs.
    ///
    /// A crashed run can leave our combine/null sink behind while it still
    /// carries the default (and the user's audio) — and a concurrently
    /// running instance's sinks look exactly the same. Unloading blindly
    /// would audibly collapse that routing, so a module whose sink is the
    /// CURRENT default is adopted instead: its id becomes tracked and, for
    /// the combine sink, the enabled set is recovered from its `slaves=`
    /// argument. Everything else of ours is orphaned and unloaded.
    fn reconcile_startup_modules(&mut self) -> anyhow::Result<()> {
        let out = run_pactl(&["list", "short", "modules"])?;
        let ours = parse_our_modules(&out);
        if ours.is_empty() {
            return Ok(());
        }
        let default = get_default_sink()?;
        let sinks = list_sinks()?;
        let mut adopted = false;
        for module in ours {
            let owns_default = module.sink_name == default
                && sinks
                    .iter()
                    .any(|s| s.owner_module == Some(u64::from(module.id)));
            if adopted || !owns_default {
                info!(
                    "unloading orphaned module #{} ({})",
                    module.id, module.sink_name
                );
                unload_module_logged(module.id);
                continue;
            }
            adopted = true;
            if module.sink_name == COMBINED_SINK {
                let existing: HashSet<&str> = sinks
                    .iter()
                    .filter(|s| !is_ours(&s.name))
                    .map(|s| s.name.as_str())
                    .collect();
                self.enabled = parse_slaves(&module.args)
                    .into_iter()
                    .filter(|slave| existing.contains(slave))
                    .map(String::from)
                    .collect();
                self.combine_module = Some(module.id);
                info!(
                    "adopted live combine module #{} over {} devices",
                    module.id,
                    self.enabled.len()
                );
            } else {
                self.null_module = Some(module.id);
                self.enabled.clear();
                info!("adopted live null-sink module #{}", module.id);
            }
        }
        Ok(())
    }

    /// Unload every module whose `sink_name=` is exactly one of our sink
    /// names. Uses the short text listing because the JSON module listing
    /// on pipewire-pulse omits module ids. Runs in cleanup(), after the
    /// tracked teardown, to catch stragglers.
    fn sweep_leftover_modules(&mut self) -> anyhow::Result<()> {
        let out = run_pactl(&["list", "short", "modules"])?;
        for module in parse_our_modules(&out) {
            info!(
                "unloading leftover module #{} ({})",
                module.id, module.sink_name
            );
            unload_module_logged(module.id);
        }
        self.combine_module = None;
        self.null_module = None;
        Ok(())
    }

    /// Best-effort unload of the modules we know we loaded. An id stays
    /// tracked when its module could not be unloaded and is still alive, so
    /// we never lose the handle to a zombie sink.
    fn unload_tracked(&mut self) {
        if let Some(id) = self.combine_module {
            if unload_module_checked(id) {
                self.combine_module = None;
            }
        }
        if let Some(id) = self.null_module {
            if unload_module_checked(id) {
                self.null_module = None;
            }
        }
    }

    /// Kill and reap the `pactl subscribe` child, then join the monitor
    /// thread. Killing the child closes its stdout, so the join is bounded
    /// by one read returning EOF (plus at most one rate-limit sleep).
    fn stop_monitor(&mut self) {
        if let Some(mut child) = self.monitor_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.monitor_thread.take() {
            if handle.join().is_err() {
                warn!("monitor thread panicked");
            }
        }
    }
}

impl AudioBackend for LinuxBackend {
    fn list_devices(&mut self) -> anyhow::Result<Vec<Device>> {
        let sinks = list_sinks()?;
        let mut devices = Vec::with_capacity(sinks.len());
        // The combine sink leads the list as the synthetic master row.
        devices.extend(master_device(self.combine_module, &sinks));
        devices.extend(
            sinks
                .into_iter()
                .filter(|s| !is_ours(&s.name))
                .map(|s| Device {
                    enabled: self.enabled.contains(&s.name),
                    id: s.name,
                    name: s.description,
                    device_type: s.device_type,
                    volume: s.volume,
                    muted: s.muted,
                }),
        );
        Ok(devices)
    }

    fn apply_enabled(&mut self, ids: &[String]) -> anyhow::Result<()> {
        let sinks = list_sinks()?;
        let existing: HashSet<&str> = sinks
            .iter()
            .filter(|s| !is_ours(&s.name))
            .map(|s| s.name.as_str())
            .collect();

        // Keep caller order, drop unknown ids and duplicates.
        let mut selected: Vec<&str> = Vec::new();
        for id in ids {
            if existing.contains(id.as_str()) && !selected.contains(&id.as_str()) {
                selected.push(id);
            }
        }

        match selected.as_slice() {
            [] => {
                // Reuse a live null sink so repeated "silence" applies are
                // no-ops. (pipewire-pulse even allows a second sink with the
                // same name, so reloading could stack duplicates.)
                let null_alive = self
                    .null_module
                    .is_some_and(|id| sinks.iter().any(|s| s.owner_module == Some(u64::from(id))));
                if !null_alive {
                    if let Some(stale) = self.null_module {
                        if !unload_module_checked(stale) {
                            bail!(
                                "null-sink module #{stale} is defunct but cannot be \
                                 unloaded; refusing to load a same-named duplicate"
                            );
                        }
                        self.null_module = None;
                    }
                    let id = load_module(&[
                        "module-null-sink",
                        &format!("sink_name={NULL_SINK}"),
                        "sink_properties=device.description='Sound-Multiplexer-Silence'",
                    ])?;
                    self.null_module = Some(id);
                }
                set_default_sink(NULL_SINK)?;
                // The combine sink goes last: the default has moved off it.
                if let Some(id) = self.combine_module {
                    if unload_module_checked(id) {
                        self.combine_module = None;
                    }
                }
                info!("routing: silence (null sink is default)");
            }
            [only] => {
                // Default moves first: never leave the system defaulting to a
                // sink we are about to destroy, so streams migrate cleanly.
                set_default_sink(only)?;
                self.unload_tracked();
                info!("routing: single device '{only}' is default");
            }
            many => {
                // Unchanged set with our combine sink still alive: nothing to
                // do. Rebuilding would cause an audible dropout, and the trait
                // requires apply_enabled to be idempotent.
                let same_set = many.len() == self.enabled.len()
                    && many.iter().all(|name| self.enabled.contains(*name));
                let combine_alive = self
                    .combine_module
                    .is_some_and(|id| sinks.iter().any(|s| s.owner_module == Some(u64::from(id))));
                if same_set && combine_alive {
                    debug!("routing: combined sink already spans the selected devices");
                    return Ok(());
                }

                // A fresh combine sink comes up at full volume; carry the
                // current master volume/mute over so a device toggle never
                // audibly jumps the overall level mid-session.
                let master_state = self.combine_module.and_then(|id| {
                    sinks
                        .iter()
                        .find(|s| s.owner_module == Some(u64::from(id)))
                        .map(|s| (s.volume, s.muted))
                });

                let slaves = many.join(",");
                let sink_name_arg = format!("sink_name={COMBINED_SINK}");
                let slaves_arg = format!("slaves={slaves}");
                let combine_args = [
                    "module-combine-sink",
                    sink_name_arg.as_str(),
                    slaves_arg.as_str(),
                    "sink_properties=device.description='Sound-Multiplexer'",
                ];

                // Build the replacement BEFORE tearing anything down: a
                // failed load then leaves the current routing (and
                // `self.enabled`) untouched. pipewire-pulse permits two sinks
                // with the same name; plain PulseAudio does not, so fall back
                // to unload-then-retry there.
                let new_id = match load_module(&combine_args) {
                    Ok(id) => id,
                    Err(first_err) => {
                        let Some(old) = self.combine_module else {
                            return Err(first_err);
                        };
                        if !unload_module_checked(old) {
                            // Routing unchanged; `self.enabled` still accurate.
                            return Err(first_err.context(
                                "could not load a replacement combine sink, and the \
                                 existing one could not be unloaded to retry",
                            ));
                        }
                        self.combine_module = None;
                        match load_module(&combine_args) {
                            Ok(id) => id,
                            Err(e) => {
                                // The old combine is gone: fall back to the
                                // first selected device that accepts being the
                                // default, so `self.enabled` matches reality.
                                self.enabled.clear();
                                if let Some(survivor) =
                                    many.iter().find(|s| set_default_sink(s).is_ok())
                                {
                                    self.enabled.insert(survivor.to_string());
                                }
                                return Err(e).context(
                                    "failed to rebuild the combined sink; \
                                     fell back to a single device",
                                );
                            }
                        }
                    }
                };

                // Retire the old combine while its name still resolves: the
                // server re-points the default (stored by name) at the
                // replacement, so streams never land on an arbitrary device.
                if let Some(old) = self.combine_module {
                    if !unload_module_checked(old) {
                        // Two live combines would fight over the name: roll
                        // the replacement back and keep the old (still
                        // routing) one tracked.
                        if !unload_module_checked(new_id) {
                            warn!(
                                "could not roll back replacement combine module \
                                 #{new_id}; cleanup will sweep it"
                            );
                        }
                        bail!("could not replace combine sink: old module #{old} would not unload");
                    }
                }
                self.combine_module = Some(new_id);
                // The old combine is gone, so the name is unique again and
                // volume/mute restoration by name hits the replacement.
                // Best-effort: a failure here must not fail the routing.
                if let Some((volume, muted)) = master_state {
                    let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u32;
                    if let Err(e) =
                        run_pactl(&["set-sink-volume", COMBINED_SINK, &format!("{percent}%")])
                    {
                        warn!("could not carry master volume over: {e:#}");
                    }
                    if muted {
                        if let Err(e) = run_pactl(&["set-sink-mute", COMBINED_SINK, "1"]) {
                            warn!("could not carry master mute over: {e:#}");
                        }
                    }
                }
                if let Err(e) = set_default_sink(COMBINED_SINK) {
                    // The combine over the new set exists and is tracked; only
                    // the default move failed. Record the new set so tracked
                    // state stays consistent, and surface the error.
                    self.enabled = many.iter().map(|s| s.to_string()).collect();
                    return Err(e);
                }
                // The null sink (0 -> 2+ transition) goes last: only now that
                // the default has moved off it does destroying it strand no
                // streams on an arbitrary device.
                if let Some(id) = self.null_module {
                    if unload_module_checked(id) {
                        self.null_module = None;
                    }
                }
                info!("routing: combined sink over {} devices", many.len());
            }
        }

        self.enabled = selected.into_iter().map(String::from).collect();
        Ok(())
    }

    fn set_volume(&mut self, id: &str, volume: f32) -> anyhow::Result<()> {
        if !volume.is_finite() {
            bail!("volume must be a finite number, got {volume}");
        }
        let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u32;
        run_pactl(&["set-sink-volume", id, &format!("{percent}%")])
            .with_context(|| format!("failed to set volume of '{id}'"))?;
        Ok(())
    }

    fn set_muted(&mut self, id: &str, muted: bool) -> anyhow::Result<()> {
        run_pactl(&["set-sink-mute", id, if muted { "1" } else { "0" }])
            .with_context(|| format!("failed to set mute of '{id}'"))?;
        Ok(())
    }

    fn start_monitoring(&mut self, tx: Sender<BackendEvent>) -> anyhow::Result<()> {
        // May be called repeatedly; each call stops and replaces the
        // previous monitor.
        self.stop_monitor();
        let mut child = pactl_command()
            .arg("subscribe")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to spawn `pactl subscribe`")?;
        let stdout = child
            .stdout
            .take()
            .context("`pactl subscribe` has no stdout pipe")?;
        self.monitor_thread = Some(std::thread::spawn(move || monitor_loop(stdout, tx)));
        self.monitor_child = Some(child);
        Ok(())
    }

    fn cleanup(&mut self) -> anyhow::Result<()> {
        // Stop the monitor first so it does not react to our own teardown.
        self.stop_monitor();

        let mut first_err: Option<anyhow::Error> = None;

        // Move the default off our sinks BEFORE destroying them.
        match (get_default_sink(), list_sinks()) {
            (Ok(default), Ok(sinks)) if is_ours(&default) => {
                let real: Vec<&str> = sinks
                    .iter()
                    .filter(|s| !is_ours(&s.name))
                    .map(|s| s.name.as_str())
                    .collect();
                let target = real
                    .iter()
                    .find(|name| self.enabled.contains(**name))
                    .or_else(|| real.first());
                if let Some(target) = target {
                    if let Err(e) = set_default_sink(target) {
                        first_err.get_or_insert(e);
                    }
                } else {
                    warn!("cleanup: no real sink available to restore as default");
                }
            }
            (Ok(_), Ok(_)) => {}
            (res_a, res_b) => {
                if let Err(e) = res_a {
                    first_err.get_or_insert(e);
                }
                if let Err(e) = res_b {
                    first_err.get_or_insert(e);
                }
            }
        }

        self.unload_tracked();
        if let Err(e) = self.sweep_leftover_modules() {
            first_err.get_or_insert(e);
        }

        match first_err {
            Some(e) => Err(e).context("cleanup left the audio setup partially restored"),
            None => Ok(()),
        }
    }
}

impl Drop for LinuxBackend {
    fn drop(&mut self) {
        // Safety net if cleanup() was never called: reap the subscribe child
        // (its EOF also ends the monitor thread). No pactl calls here.
        if let Some(mut child) = self.monitor_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// pactl plumbing
// ---------------------------------------------------------------------------

/// Base `pactl` command. LC_ALL=C on every invocation: `pactl subscribe`
/// output is gettext-localized (the monitor's English matchers would go
/// silently dead on other locales), and pactl 16's JSON output uses the
/// locale's decimal separator, which breaks JSON parsing.
fn pactl_command() -> Command {
    let mut cmd = Command::new("pactl");
    cmd.env("LC_ALL", "C");
    cmd
}

fn run_pactl(args: &[&str]) -> anyhow::Result<String> {
    let output = pactl_command().args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("pactl not found -- install pipewire-pulse or pulseaudio-utils")
        } else {
            anyhow::Error::new(e).context("failed to run pactl")
        }
    })?;
    if !output.status.success() {
        bail!(
            "`pactl {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn list_sinks() -> anyhow::Result<Vec<Sink>> {
    parse_sinks(&run_pactl(&["-f", "json", "list", "sinks"])?)
}

fn get_default_sink() -> anyhow::Result<String> {
    Ok(run_pactl(&["get-default-sink"])?.trim().to_string())
}

fn set_default_sink(name: &str) -> anyhow::Result<()> {
    run_pactl(&["set-default-sink", name])
        .with_context(|| format!("failed to make '{name}' the default sink"))?;
    Ok(())
}

/// `pactl load-module <args>`; pactl prints the new module id on stdout.
fn load_module(args: &[&str]) -> anyhow::Result<u32> {
    let mut argv = vec!["load-module"];
    argv.extend_from_slice(args);
    let out = run_pactl(&argv)?;
    let id = parse_module_id(&out)?;
    debug!("loaded module #{id}: {}", args.join(" "));
    Ok(id)
}

fn unload_module_logged(id: u32) {
    if let Err(e) = run_pactl(&["unload-module", &id.to_string()]) {
        // Stale id (module already gone) is expected after external changes.
        warn!("could not unload module #{id}: {e:#}");
    }
}

/// Unload a module and report whether it is verifiably gone: unloaded now,
/// or its id no longer listed (stale id after external changes). Returns
/// false when the module may still be alive; the caller must then keep
/// tracking the id, or a zombie sink would survive that the app can no
/// longer manage (pipewire-pulse even allows loading a same-named
/// duplicate next to it).
fn unload_module_checked(id: u32) -> bool {
    let err = match run_pactl(&["unload-module", &id.to_string()]) {
        Ok(_) => return true,
        Err(e) => e,
    };
    match run_pactl(&["list", "short", "modules"]) {
        Ok(out) => {
            let alive = listed_module_ids(&out).contains(&id);
            if alive {
                warn!("could not unload module #{id}, it is still loaded: {err:#}");
            } else {
                debug!("unload of module #{id} failed but it is already gone: {err:#}");
            }
            !alive
        }
        Err(list_err) => {
            warn!(
                "could not unload module #{id} ({err:#}) nor verify it \
                 ({list_err:#}); keeping it tracked"
            );
            false
        }
    }
}

fn is_ours(sink_name: &str) -> bool {
    sink_name.starts_with(OUR_PREFIX)
}

/// The live combine sink as the synthetic "Master volume" device, present
/// only while 2+ devices are routed through it. Matched by owner module —
/// sink names are not unique on pipewire-pulse, module ids are. Volume and
/// mute are real (the sink's own), and apply upstream of every slave; this
/// is also exactly what the system volume UI controls while the combine
/// sink is the default.
fn master_device(combine_module: Option<u32>, sinks: &[Sink]) -> Option<Device> {
    let module = combine_module?;
    let sink = sinks
        .iter()
        .find(|s| s.owner_module == Some(u64::from(module)))?;
    Some(Device {
        id: sink.name.clone(),
        name: "Master volume".to_string(),
        device_type: DeviceType::Master,
        enabled: true,
        volume: sink.volume,
        muted: sink.muted,
    })
}

fn parse_module_id(load_module_stdout: &str) -> anyhow::Result<u32> {
    load_module_stdout
        .trim()
        .parse()
        .with_context(|| format!("unexpected `pactl load-module` output: {load_module_stdout:?}"))
}

/// One of our modules found in `pactl list short modules`.
#[derive(Debug, PartialEq, Eq)]
struct OurModule {
    id: u32,
    /// Exact `sink_name=` argument value: COMBINED_SINK or NULL_SINK.
    sink_name: String,
    /// Full argument string (holds `slaves=` for the combine sink).
    args: String,
}

/// Extract our modules from `pactl list short modules`, matched by their
/// exact `sink_name=` argument so no foreign module is ever swept.
///
/// Native PipeWire modules print multi-line `{ ... }` arguments in this
/// listing; only lines whose first tab-separated field is a numeric id are
/// module rows, the rest are argument continuations.
fn parse_our_modules(short_modules: &str) -> Vec<OurModule> {
    short_modules
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let id: u32 = fields.next()?.trim().parse().ok()?;
            let name = fields.next()?.trim();
            if !name.starts_with("module-") {
                return None;
            }
            let args = fields.next().unwrap_or("").trim();
            let sink_name = arg_value(args, "sink_name")?;
            (sink_name == COMBINED_SINK || sink_name == NULL_SINK).then(|| OurModule {
                id,
                sink_name: sink_name.to_string(),
                args: args.to_string(),
            })
        })
        .collect()
}

/// Every module id in `pactl list short modules` output (argument
/// continuation lines of native modules never parse as an id).
fn listed_module_ids(short_modules: &str) -> Vec<u32> {
    short_modules
        .lines()
        .filter_map(|line| line.split('\t').next()?.trim().parse().ok())
        .collect()
}

/// Value of a `key=value` token in a module argument string. The values we
/// read from our own modules never contain whitespace.
fn arg_value<'a>(args: &'a str, key: &str) -> Option<&'a str> {
    args.split_whitespace()
        .find_map(|token| token.strip_prefix(key)?.strip_prefix('='))
}

/// Slave sink names from a module-combine-sink argument string, used when
/// adopting a still-live combine sink at startup.
fn parse_slaves(args: &str) -> Vec<&str> {
    arg_value(args, "slaves")
        .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Sink enumeration (pactl -f json list sinks)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct RawSink {
    name: String,
    description: String,
    #[serde(default)]
    mute: bool,
    /// Id of the module that owns this sink; PA_INVALID_INDEX when none.
    #[serde(default)]
    owner_module: Option<u64>,
    /// Keyed by channel name ("front-left", ...).
    #[serde(default)]
    volume: HashMap<String, RawChannelVolume>,
    #[serde(default)]
    properties: serde_json::Map<String, serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct RawChannelVolume {
    /// Raw PulseAudio volume; 65536 == 100%.
    value: u64,
}

#[derive(Debug, Clone)]
struct Sink {
    name: String,
    description: String,
    /// Owning module id, if any. Links a sink back to the module we loaded
    /// (sink names are NOT unique on pipewire-pulse, module ids are).
    owner_module: Option<u64>,
    volume: f32,
    muted: bool,
    device_type: DeviceType,
}

fn parse_sinks(json: &str) -> anyhow::Result<Vec<Sink>> {
    let raw: Vec<RawSink> =
        serde_json::from_str(json).context("unexpected `pactl -f json list sinks` output")?;
    Ok(raw
        .into_iter()
        .map(|sink| {
            let volume = if sink.volume.is_empty() {
                1.0
            } else {
                let sum: f64 = sink.volume.values().map(|c| c.value as f64 / 65536.0).sum();
                ((sum / sink.volume.len() as f64) as f32).clamp(0.0, 1.0)
            };
            let bus = sink.properties.get("device.bus").and_then(|v| v.as_str());
            let form_factor = sink
                .properties
                .get("device.form_factor")
                .and_then(|v| v.as_str());
            let device_type = infer_device_type(&sink.name, &sink.description, bus, form_factor);
            Sink {
                name: sink.name,
                description: sink.description,
                owner_module: sink.owner_module.filter(|&m| m != PA_INVALID_INDEX),
                volume,
                muted: sink.mute,
                device_type,
            }
        })
        .collect())
}

/// Classify a sink from its name, description and udev properties.
/// Priority matters: Bluetooth headphones must classify as Bluetooth, and a
/// USB headset as Headphones.
fn infer_device_type(
    name: &str,
    description: &str,
    bus: Option<&str>,
    form_factor: Option<&str>,
) -> DeviceType {
    let name = name.to_lowercase();
    let description = description.to_lowercase();
    let bus = bus.map(str::to_lowercase);
    let form_factor = form_factor.map(str::to_lowercase);
    let mentions = |keyword: &str| name.contains(keyword) || description.contains(keyword);

    if bus.as_deref() == Some("bluetooth") || name.contains("bluez") {
        DeviceType::Bluetooth
    } else if form_factor.as_deref() == Some("headphone")
        || mentions("headphone")
        || mentions("headset")
    {
        DeviceType::Headphones
    } else if mentions("hdmi") || mentions("displayport") {
        DeviceType::Hdmi
    } else if mentions("iec958") || mentions("spdif") || mentions("digital") {
        DeviceType::Digital
    } else if bus.as_deref() == Some("usb") {
        DeviceType::Usb
    } else {
        DeviceType::Speakers
    }
}

// ---------------------------------------------------------------------------
// Change monitoring (pactl subscribe)
// ---------------------------------------------------------------------------

/// Cached view the monitor thread diffs against: per-sink volume/mute of the
/// real sinks plus our combine sink (the master row tracks external master
/// changes, e.g. the system volume UI while the combine sink is default),
/// plus the default sink name (so external routing changes surface as
/// DevicesChanged).
struct Snapshot {
    state: HashMap<String, (f32, bool)>,
    default_sink: String,
}

fn take_snapshot() -> anyhow::Result<Snapshot> {
    let sinks = list_sinks()?;
    let default_sink = get_default_sink().unwrap_or_default();
    Ok(Snapshot {
        state: sinks
            .into_iter()
            .filter(|s| !is_ours(&s.name) || s.name == COMBINED_SINK)
            .map(|s| (s.name, (s.volume, s.muted)))
            .collect(),
        default_sink,
    })
}

/// True for the `pactl subscribe` lines the monitor reacts to: sink events
/// (e.g. "Event 'change' on sink #67"; the "#" excludes sink-inputs) and
/// server events (default-sink changes). Localized output never matches,
/// which is why every pactl child runs under LC_ALL=C.
fn is_sink_or_server_event(line: &str) -> bool {
    line.contains(" on sink #") || line.contains(" on server")
}

/// Turn `pactl subscribe` lines into [`BackendEvent`]s by re-enumerating and
/// diffing snapshots. Exits on child EOF/pipe error (how `stop_monitor`
/// bounds its join) or when the receiver is gone.
fn monitor_loop(stdout: ChildStdout, tx: Sender<BackendEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut snapshot = take_snapshot().ok();
    let mut last_enum = Instant::now();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break, // child killed or pipe broken
            Ok(_) => {}
        }
        if !is_sink_or_server_event(&line) {
            continue;
        }
        // Rate limit: sleeping (instead of skipping) keeps the last event of
        // a burst, so the final state of e.g. a volume drag is never lost.
        let elapsed = last_enum.elapsed();
        if elapsed < REENUM_MIN_INTERVAL {
            std::thread::sleep(REENUM_MIN_INTERVAL - elapsed);
        }
        last_enum = Instant::now();

        let new = match take_snapshot() {
            Ok(s) => s,
            Err(e) => {
                // Transient during device hotplug; keep the old snapshot.
                warn!("monitor: re-enumeration failed: {e:#}");
                continue;
            }
        };
        let sent_ok = match &snapshot {
            Some(old) => send_diff(old, &new, &tx),
            None => tx.send(BackendEvent::DevicesChanged).is_ok(),
        };
        snapshot = Some(new);
        if !sent_ok {
            break; // receiver dropped
        }
    }
    debug!("pactl subscribe monitor thread exiting");
}

/// Emit events for the differences between two snapshots.
/// Returns false when the receiver is gone.
fn send_diff(old: &Snapshot, new: &Snapshot, tx: &Sender<BackendEvent>) -> bool {
    let set_changed = old.state.len() != new.state.len()
        || !old.state.keys().all(|name| new.state.contains_key(name));
    if set_changed || old.default_sink != new.default_sink {
        // Coarse event; the app re-lists everything, so per-device diffs
        // would be redundant here.
        return tx.send(BackendEvent::DevicesChanged).is_ok();
    }
    for (name, (volume, muted)) in &new.state {
        let Some((old_volume, old_muted)) = old.state.get(name) else {
            continue;
        };
        if (volume - old_volume).abs() > 0.01 {
            let event = BackendEvent::VolumeChanged {
                id: name.clone(),
                volume: *volume,
            };
            if tx.send(event).is_err() {
                return false;
            }
        }
        if muted != old_muted {
            let event = BackendEvent::MuteChanged {
                id: name.clone(),
                muted: *muted,
            };
            if tx.send(event).is_err() {
                return false;
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed capture of `pactl -f json list sinks` (pactl 17 on
    /// pipewire-pulse), extended with representative synthetic sinks.
    const SINKS_JSON: &str = r#"[
      {
        "index": 67,
        "state": "SUSPENDED",
        "name": "alsa_output.pci-0000_00_1f.3.analog-stereo",
        "description": "Built-in Audio Analog Stereo",
        "driver": "PipeWire",
        "owner_module": 4294967295,
        "mute": true,
        "volume": {
          "front-left": { "value": 32768, "value_percent": "50%", "db": "-18.06 dB" },
          "front-right": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" }
        },
        "balance": 0.0,
        "properties": {
          "device.bus": "pci",
          "device.form_factor": "internal",
          "media.class": "Audio/Sink"
        },
        "active_port": "analog-output-speaker"
      },
      {
        "index": 70,
        "name": "bluez_output.AA_BB_CC_DD_EE_FF.1",
        "description": "WH-1000XM4",
        "mute": false,
        "volume": {
          "mono": { "value": 98304, "value_percent": "150%", "db": "10.57 dB" }
        },
        "properties": {
          "device.bus": "bluetooth",
          "device.form_factor": "headset"
        }
      },
      {
        "index": 71,
        "name": "alsa_output.pci-0000_01_00.1.hdmi-stereo",
        "description": "HDMI Audio",
        "mute": false,
        "volume": {},
        "properties": {}
      },
      {
        "index": 90,
        "name": "sound_multiplexer_combined",
        "description": "Sound-Multiplexer",
        "owner_module": 536870916,
        "mute": false,
        "volume": {
          "front-left": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" }
        },
        "properties": {}
      }
    ]"#;

    #[test]
    fn parses_sink_json() {
        let sinks = parse_sinks(SINKS_JSON).unwrap();
        assert_eq!(sinks.len(), 4);

        let builtin = &sinks[0];
        assert_eq!(builtin.name, "alsa_output.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(builtin.description, "Built-in Audio Analog Stereo");
        assert!(builtin.muted);
        // Mean of 50% and 100%.
        assert!((builtin.volume - 0.75).abs() < 0.001);
        assert_eq!(builtin.device_type, DeviceType::Speakers);

        let bt = &sinks[1];
        assert!(!bt.muted);
        // 150% clamps to 1.0 for the Device contract.
        assert_eq!(bt.volume, 1.0);
        assert_eq!(bt.device_type, DeviceType::Bluetooth);

        // No channels reported -> assume full volume.
        assert_eq!(sinks[2].volume, 1.0);
        assert_eq!(sinks[2].device_type, DeviceType::Hdmi);

        // owner_module: PA_INVALID_INDEX and a missing field mean "none";
        // our combine sink links back to the module that loaded it.
        assert_eq!(builtin.owner_module, None);
        assert_eq!(sinks[1].owner_module, None);
        assert_eq!(sinks[3].owner_module, Some(536870916));
    }

    /// pactl versions differ in which fields they emit; everything except
    /// `name` and `description` must be optional and default sanely.
    #[test]
    fn parses_sink_json_with_missing_optional_fields() {
        let json = r#"[{ "name": "alsa_output.pci-0000_00_1b.0.analog-stereo",
                         "description": "Bare Sink" }]"#;
        let sinks = parse_sinks(json).unwrap();
        assert_eq!(sinks.len(), 1);
        let sink = &sinks[0];
        assert_eq!(sink.name, "alsa_output.pci-0000_00_1b.0.analog-stereo");
        assert_eq!(sink.description, "Bare Sink");
        assert!(!sink.muted);
        assert_eq!(sink.volume, 1.0);
        assert_eq!(sink.owner_module, None);
        // No properties at all -> no bus/form_factor hints -> fallback type.
        assert_eq!(sink.device_type, DeviceType::Speakers);
    }

    /// No sinks (e.g. all card profiles off) is a valid, non-error state.
    #[test]
    fn parses_empty_sink_listing() {
        assert!(parse_sinks("[]").unwrap().is_empty());
        assert!(parse_sinks(" [ ] ").unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_sink_json() {
        // pactl < 16 has no -f json and prints the text listing instead.
        assert!(parse_sinks("Sink #67\n\tState: SUSPENDED\n").is_err());
        assert!(parse_sinks("").is_err());
        // An object where the sink array is expected.
        assert!(parse_sinks(r#"{"name": "x"}"#).is_err());
        // Output truncated mid-write (pipe cut off).
        assert!(parse_sinks(r#"[{"name": "x", "descr"#).is_err());
        // A sink entry without the required fields.
        assert!(parse_sinks(r#"[{"index": 67}]"#).is_err());
    }

    #[test]
    fn master_row_appears_only_for_the_tracked_live_combine() {
        let sinks = parse_sinks(SINKS_JSON).unwrap();

        // Tracked module with a live sink -> master row with real state.
        let master = master_device(Some(536870916), &sinks).unwrap();
        assert_eq!(master.id, COMBINED_SINK);
        assert_eq!(master.device_type, DeviceType::Master);
        assert_eq!(master.name, "Master volume");
        assert!(master.enabled);
        assert!(!master.muted);
        assert_eq!(master.volume, 1.0);

        // No tracked module (0/1-device routing) -> no master row.
        assert!(master_device(None, &sinks).is_none());
        // Tracked module whose sink is gone (mid-rebuild) -> no master row.
        assert!(master_device(Some(999), &sinks).is_none());
    }

    #[test]
    fn own_sinks_are_recognized() {
        assert!(is_ours("sound_multiplexer_combined"));
        assert!(is_ours("sound_multiplexer_null"));
        assert!(!is_ours("alsa_output.pci-0000_00_1f.3.analog-stereo"));
    }

    #[test]
    fn infers_device_types_by_priority() {
        use DeviceType::*;
        // Bluetooth wins over headphone hints.
        assert_eq!(
            infer_device_type("bluez_output.X.1", "WH-1000XM4 Headphones", Some("bluetooth"), Some("headset")),
            Bluetooth
        );
        assert_eq!(infer_device_type("BLUEZ_output.Y", "Speaker", None, None), Bluetooth);
        // Headphones via form factor or keywords, beating USB bus.
        assert_eq!(infer_device_type("alsa_output.usb-Foo", "Gaming Headset", Some("usb"), None), Headphones);
        assert_eq!(infer_device_type("alsa_output.pci-1", "Built-in", Some("pci"), Some("headphone")), Headphones);
        // HDMI / DisplayPort.
        assert_eq!(infer_device_type("alsa_output.pci-2.hdmi-stereo", "GPU 41 HDMI", Some("pci"), None), Hdmi);
        assert_eq!(infer_device_type("alsa_output.pci-2.3", "DisplayPort Audio", None, None), Hdmi);
        // Digital outputs.
        assert_eq!(infer_device_type("alsa_output.pci-3.iec958-stereo", "Digital Out", Some("pci"), None), Digital);
        assert_eq!(infer_device_type("alsa_output.x", "SPDIF Output", None, None), Digital);
        // Plain USB DAC.
        assert_eq!(infer_device_type("alsa_output.usb-DAC", "Audio DAC", Some("usb"), None), Usb);
        // Fallback.
        assert_eq!(infer_device_type("alsa_output.pci-0000_00_1f.3.analog-stereo", "Built-in Audio", Some("pci"), Some("internal")), Speakers);
    }

    #[test]
    fn finds_our_modules_in_short_listing() {
        // Real pipewire-pulse output mixes single-line pactl-loaded modules
        // with native modules whose arguments span multiple lines. Matching
        // is by exact sink name: foreign sinks — even ones sharing our
        // prefix — must never be swept.
        let listing = "1\tlibpipewire-module-rt\t{\n\
            \x20           nice.level    = -11\n\
            \x20           rt.prio       = 60\n\
            \x20       }\t\n\
            2\tlibpipewire-module-protocol-native\t\n\
            536870916\tmodule-combine-sink\tsink_name=sound_multiplexer_combined slaves=a,b sink_properties=device.description='Sound-Multiplexer'\t\n\
            536870917\tmodule-null-sink\tsink_name=sound_multiplexer_null\t\n\
            536870918\tmodule-null-sink\tsink_name=someone_elses_null\t\n\
            536870919\tmodule-null-sink\tsink_name=sound_multiplexer_nullish\t\n";
        let ours = parse_our_modules(listing);
        assert_eq!(ours.len(), 2);
        assert_eq!(ours[0].id, 536870916);
        assert_eq!(ours[0].sink_name, COMBINED_SINK);
        assert_eq!(parse_slaves(&ours[0].args), vec!["a", "b"]);
        assert_eq!(ours[1].id, 536870917);
        assert_eq!(ours[1].sink_name, NULL_SINK);
        assert!(parse_slaves(&ours[1].args).is_empty());
    }

    #[test]
    fn lists_every_module_id() {
        let listing = "1\tlibpipewire-module-rt\t{\n\
            \x20           nice.level    = -11\n\
            \x20       }\t\n\
            536870916\tmodule-combine-sink\tsink_name=sound_multiplexer_combined slaves=a\t\n";
        assert_eq!(listed_module_ids(listing), vec![1, 536870916]);
    }

    #[test]
    fn extracts_module_argument_values() {
        let args = "sink_name=sound_multiplexer_combined \
                    slaves=alsa_output.pci-1,bluez_output.AA.1 \
                    sink_properties=device.description='Sound-Multiplexer'";
        assert_eq!(arg_value(args, "sink_name"), Some(COMBINED_SINK));
        assert_eq!(
            arg_value(args, "slaves"),
            Some("alsa_output.pci-1,bluez_output.AA.1")
        );
        // A key must match a whole token, not a prefix of another key.
        assert_eq!(arg_value(args, "sink"), None);
        assert_eq!(arg_value(args, "slave"), None);
        assert_eq!(arg_value("", "sink_name"), None);
    }

    #[test]
    fn module_argument_quoting_edge_cases() {
        // A quoted value with whitespace in an earlier argument must not
        // hide a later key (whitespace splitting cuts the quoted value, but
        // the following key=value tokens still parse).
        let args = "sink_properties=device.description='Nice Name' \
                    sink_name=sound_multiplexer_null";
        assert_eq!(arg_value(args, "sink_name"), Some(NULL_SINK));
        // Tabs separate arguments just as well as spaces.
        assert_eq!(arg_value("sink_name=a\tslaves=b,c", "slaves"), Some("b,c"));
        // First occurrence wins for a repeated key.
        assert_eq!(arg_value("slaves=a slaves=b", "slaves"), Some("a"));
        // Values may themselves contain '='.
        assert_eq!(
            arg_value("sink_properties=device.description='X'", "sink_properties"),
            Some("device.description='X'")
        );

        // We always load with an unquoted sink_name, so a quoted one is
        // foreign and the sweep must stay conservative and skip it. A
        // native module's argument-continuation line mentioning our sink
        // name is not a module row either.
        let listing = "7\tmodule-null-sink\tsink_name=\"sound_multiplexer_null\"\t\n\
            \x20   sink_name=sound_multiplexer_combined\n\
            8\tmodule-null-sink\tsink_properties=device.description='a b' sink_name=sound_multiplexer_null\t\n";
        let ours = parse_our_modules(listing);
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].id, 8);
        assert_eq!(ours[0].sink_name, NULL_SINK);
    }

    #[test]
    fn monitor_matches_sink_and_server_lines_only() {
        // Sink lifecycle and property changes drive re-enumeration.
        assert!(is_sink_or_server_event("Event 'change' on sink #67\n"));
        assert!(is_sink_or_server_event("Event 'new' on sink #123\n"));
        assert!(is_sink_or_server_event("Event 'remove' on sink #5\n"));
        // Default-sink changes arrive as server events.
        assert!(is_sink_or_server_event("Event 'change' on server #0\n"));
        // Stream (sink-input) chatter is constant during playback and must
        // not trigger re-enumeration.
        assert!(!is_sink_or_server_event("Event 'change' on sink-input #45\n"));
        assert!(!is_sink_or_server_event("Event 'remove' on sink-input #45\n"));
        // Inputs are out of scope for an output multiplexer.
        assert!(!is_sink_or_server_event("Event 'change' on source #52\n"));
        assert!(!is_sink_or_server_event("Event 'new' on source-output #8\n"));
        assert!(!is_sink_or_server_event("Event 'change' on client #99\n"));
        assert!(!is_sink_or_server_event("Event 'change' on card #46\n"));
        // Localized output (as without LC_ALL=C) must never silently match.
        assert!(!is_sink_or_server_event("Ereignis 'change' für Senke #67\n"));
        assert!(!is_sink_or_server_event(""));
    }

    /// Startup adoption recovers the enabled set from a live combine
    /// module's `slaves=` argument.
    #[test]
    fn parses_slaves_for_adoption() {
        let args = "sink_name=sound_multiplexer_combined \
                    slaves=alsa_output.pci-1,bluez_output.AA.1 \
                    sink_properties=device.description='Sound-Multiplexer'";
        assert_eq!(
            parse_slaves(args),
            vec!["alsa_output.pci-1", "bluez_output.AA.1"]
        );
        assert_eq!(parse_slaves("sink_name=x slaves=only_one"), vec!["only_one"]);
        assert!(parse_slaves("sink_name=sound_multiplexer_null").is_empty());
        assert!(parse_slaves("sink_name=x slaves=").is_empty());
        assert!(parse_slaves("").is_empty());
    }

    #[test]
    fn parses_load_module_stdout() {
        assert_eq!(parse_module_id("536870916\n").unwrap(), 536870916);
        assert!(parse_module_id("Failure: Module initialization failed").is_err());
        assert!(parse_module_id("").is_err());
    }

    /// End-to-end against the real sound server: `cargo test -- --ignored`.
    /// Loads a combine sink slaved to the current default, checks it is
    /// enumerated and filtered as ours, unloads it. Never changes the
    /// default sink or any real sink's volume/mute.
    #[test]
    #[ignore = "requires a running PulseAudio/PipeWire server"]
    fn live_smoke() {
        run_pactl(&["info"]).expect("sound server unreachable");
        let default_before = get_default_sink().unwrap();
        assert!(!default_before.is_empty() && !is_ours(&default_before));

        let id = load_module(&[
            "module-combine-sink",
            "sink_name=sound_multiplexer_livetest",
            &format!("slaves={default_before}"),
            "sink_properties=device.description='Sound-Multiplexer-Livetest'",
        ])
        .unwrap();

        let listing = run_pactl(&["list", "short", "modules"]).unwrap();
        assert!(listed_module_ids(&listing).contains(&id));
        // The livetest sink shares our prefix but is not one of our exact
        // sink names: the sweep matcher must not claim it.
        assert!(parse_our_modules(&listing).iter().all(|m| m.id != id));

        let sinks = list_sinks().unwrap();
        let ours = sinks
            .iter()
            .find(|s| s.name == "sound_multiplexer_livetest")
            .expect("combine sink not enumerated");
        assert!(is_ours(&ours.name));
        assert_eq!(ours.description, "Sound-Multiplexer-Livetest");
        // The JSON sink listing links the sink back to the module we
        // loaded — the alive-checks in apply_enabled depend on this.
        assert_eq!(ours.owner_module, Some(u64::from(id)));

        assert!(unload_module_checked(id));
        // A second unload of the now-stale id must also report "gone".
        assert!(unload_module_checked(id));

        assert_eq!(get_default_sink().unwrap(), default_before);
        let listing = run_pactl(&["list", "short", "modules"]).unwrap();
        assert!(!listed_module_ids(&listing).contains(&id));
    }
}
