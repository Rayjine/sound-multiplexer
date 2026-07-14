//! Platform audio backends for Sound Multiplexer.
//!
//! The app-facing contract is [`AudioBackend`]: enumerate output devices,
//! apply "the set of devices audio should play on" atomically, control
//! per-device volume/mute, and push change notifications from a background
//! thread. Routing strategy per enabled-set size (both platforms):
//!   0 devices  -> true silence (Linux: null sink as default; Windows: mute default endpoint)
//!   1 device   -> plain default device, no extra plumbing
//!   2+ devices -> platform fan-out (Linux: module-combine-sink; Windows: loopback engine)

use std::sync::mpsc::Sender;

pub mod fanout;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Speakers,
    Headphones,
    Bluetooth,
    Hdmi,
    Usb,
    Digital,
    /// The app's own combined output, surfaced as a "Master volume" row
    /// while 2+ devices are enabled. Never part of an enabled set; its
    /// volume/mute apply upstream of every per-device control.
    Master,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Device {
    /// Stable platform identifier (Linux: sink name; Windows: endpoint ID).
    pub id: String,
    /// Human-readable name shown in the UI.
    pub name: String,
    #[serde(rename = "deviceType")]
    pub device_type: DeviceType,
    /// Audio plays on this device (part of the applied enabled set).
    pub enabled: bool,
    /// 0.0..=1.0
    pub volume: f32,
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
    VolumeChanged { id: String, volume: f32 },
    /// A device's mute state changed outside the app.
    MuteChanged { id: String, muted: bool },
    /// Non-fatal backend problem worth surfacing to the user.
    Error(String),
}

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
