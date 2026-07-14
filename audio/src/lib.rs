//! Platform audio backends for Sound Multiplexer.
//!
//! The entry point is [`create_backend`]; everything the app needs from a
//! platform lives behind the [`AudioBackend`] trait.
//!
//! # The `AudioBackend` contract
//!
//! Routing is stateless from the caller's side: [`AudioBackend::apply_enabled`]
//! receives the complete set of device ids audio should play on and makes the
//! system match, idempotently. Per enabled-set size (both platforms):
//!
//! - 0 devices — true silence. Linux makes a null sink the default; Windows
//!   mutes the default endpoint (and only unmutes it again if the mute was
//!   the app's own, never a user's).
//! - 1 device — that device becomes the plain system default; no extra
//!   plumbing exists to add latency or leak on a crash.
//! - 2+ devices — platform fan-out. Linux loads `module-combine-sink`;
//!   Windows loopback-captures the primary endpoint and re-renders the
//!   stream to the other enabled devices.
//!
//! While a fan-out is active, [`AudioBackend::list_devices`] prepends a
//! synthetic "Master volume" row ([`DeviceType::Master`]): the fan-out sink's
//! own, real volume/mute, applied upstream of every per-device control. It is
//! presentation only and never part of an enabled set
//! ([`compute_enabled_ids`] filters it out).
//!
//! Events: [`AudioBackend::start_monitoring`] starts a monitor thread that
//! owns the given channel and reports external changes as [`BackendEvent`]s.
//! Payloads are deliberately coarse — the app responds by re-listing devices
//! and pushing full state to the UI, so an event only needs to say that
//! something changed. Monitoring is restart-safe: a repeated
//! `start_monitoring` call stops and replaces the previous monitor, which is
//! how the app revives its subscription after a sound-server restart.
//!
//! Cleanup: [`AudioBackend::cleanup`] removes every routing artifact the app
//! created and leaves a real device as the system default. Backends also
//! recover from a previous run's leftovers at construction time (see
//! [`create_backend`]).
//!
//! # Platform coverage
//!
//! Linux (`pactl` driving PulseAudio or pipewire-pulse) is the proven
//! backend: unit tests, an `--ignored` live E2E test against a real sound
//! server, and real-world use. Windows (WASAPI) compiles and its pure logic
//! is unit-tested, but it has never run on real hardware — treat it as
//! unverified.

#![warn(missing_docs)]

use std::sync::mpsc::Sender;

pub mod fanout;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

/// Coarse device category, inferred from platform metadata by keyword and
/// form-factor heuristics. Priority order is part of the contract and
/// matches across platforms: Bluetooth (transport) beats Headphones (form
/// factor) beats Usb (transport).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    /// Fallback when nothing more specific matches (typically built-in
    /// analog outputs).
    Speakers,
    /// Wired headphones or headsets, by form factor or name.
    Headphones,
    /// Any Bluetooth output — including BT headphones; the transport wins.
    Bluetooth,
    /// HDMI or DisplayPort audio.
    Hdmi,
    /// USB audio not classified as anything more specific.
    Usb,
    /// S/PDIF / IEC958 or other digital output.
    Digital,
    /// The app's own combined output, surfaced as a "Master volume" row
    /// while 2+ devices are enabled. Never part of an enabled set; its
    /// volume/mute apply upstream of every per-device control.
    Master,
}

/// One audio output as the app and UI see it: identity plus live state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Device {
    /// Stable platform identifier (Linux: sink name; Windows: endpoint ID).
    pub id: String,
    /// Human-readable name shown in the UI.
    pub name: String,
    /// Coarse category; see [`DeviceType`].
    #[serde(rename = "deviceType")]
    pub device_type: DeviceType,
    /// Audio plays on this device (part of the applied enabled set).
    pub enabled: bool,
    /// 0.0..=1.0
    pub volume: f32,
    /// The device's own mute switch, as reported by the platform.
    pub muted: bool,
}

/// Notifications pushed by the backend's monitor thread. Payloads are
/// intentionally coarse: on any of these the app re-lists devices and
/// pushes the full state to the UI.
#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// Device added/removed or routing changed externally.
    DevicesChanged,
    /// A device's volume changed outside the app (system mixer, hardware keys).
    VolumeChanged {
        /// [`Device::id`] of the affected device.
        id: String,
        /// New volume, 0.0..=1.0.
        volume: f32,
    },
    /// A device's mute state changed outside the app.
    MuteChanged {
        /// [`Device::id`] of the affected device.
        id: String,
        /// New mute state.
        muted: bool,
    },
    /// Non-fatal backend problem worth surfacing to the user.
    Error(String),
}

/// Platform routing driver; the crate docs spell out the full contract
/// (routing semantics, master row, event flow, cleanup guarantees).
/// `Send` because the app moves it between threads; every method takes
/// `&mut self`, so calls are externally serialized and implementations
/// need no internal locking.
pub trait AudioBackend: Send {
    /// Enumerate current output devices, excluding any device this app
    /// created. `enabled` reflects the currently applied set.
    fn list_devices(&mut self) -> anyhow::Result<Vec<Device>>;

    /// Apply the full enabled set in one routing update. Ids not present
    /// anymore are ignored. Must be idempotent.
    fn apply_enabled(&mut self, ids: &[String]) -> anyhow::Result<()>;

    /// Set a device's volume (0.0..=1.0) on the physical device.
    fn set_volume(&mut self, id: &str, volume: f32) -> anyhow::Result<()>;

    /// Mute/unmute the physical device.
    fn set_muted(&mut self, id: &str, muted: bool) -> anyhow::Result<()>;

    /// Start the monitor thread; it owns `tx` until `cleanup`.
    /// May be called repeatedly; each call stops and replaces the
    /// previous monitor (used to revive monitoring after a sound-server
    /// restart).
    fn start_monitoring(&mut self, tx: Sender<BackendEvent>) -> anyhow::Result<()>;

    /// Tear down all routing this app created and stop monitoring.
    /// Called on exit; must leave the system on a sane default device.
    fn cleanup(&mut self) -> anyhow::Result<()>;
}

/// The enabled-id set that results from toggling `id` to `enabled`,
/// preserving the relative order of the other enabled devices. The
/// synthetic master row is never part of an enabled set.
pub fn compute_enabled_ids(devices: &[Device], id: &str, enabled: bool) -> Vec<String> {
    let mut ids: Vec<String> = devices
        .iter()
        .filter(|d| d.enabled && d.id != id && d.device_type != DeviceType::Master)
        .map(|d| d.id.clone())
        .collect();
    if enabled {
        ids.push(id.to_string());
    }
    ids
}

/// Construct the backend for the current platform. On startup the backend
/// removes any leftovers from a previous crashed run, then treats the
/// current system default device as the initially enabled device.
pub fn create_backend() -> anyhow::Result<Box<dyn AudioBackend>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxBackend::new()?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsBackend::new()?))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        anyhow::bail!("unsupported platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, enabled: bool) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            device_type: DeviceType::Speakers,
            enabled,
            volume: 1.0,
            muted: false,
        }
    }

    #[test]
    fn toggling_on_appends_preserving_order() {
        let devices = [dev("a", true), dev("b", false), dev("c", true)];
        assert_eq!(compute_enabled_ids(&devices, "b", true), ["a", "c", "b"]);
    }

    #[test]
    fn toggling_off_removes_only_that_id() {
        let devices = [dev("a", true), dev("b", true)];
        assert_eq!(compute_enabled_ids(&devices, "a", false), ["b"]);
    }

    #[test]
    fn toggling_on_an_already_enabled_id_does_not_duplicate() {
        let devices = [dev("a", true), dev("b", true)];
        assert_eq!(compute_enabled_ids(&devices, "b", true), ["a", "b"]);
    }

    #[test]
    fn unknown_id_toggle_off_is_a_no_op() {
        let devices = [dev("a", true)];
        assert_eq!(compute_enabled_ids(&devices, "ghost", false), ["a"]);
    }

    #[test]
    fn master_row_never_joins_an_enabled_set() {
        let mut master = dev("sound_multiplexer_combined", true);
        master.device_type = DeviceType::Master;
        let devices = [master, dev("a", true), dev("b", false)];
        assert_eq!(compute_enabled_ids(&devices, "b", true), ["a", "b"]);
        assert_eq!(compute_enabled_ids(&devices, "a", false), Vec::<String>::new());
    }
}
