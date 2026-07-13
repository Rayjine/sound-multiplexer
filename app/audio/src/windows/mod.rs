//! Windows backend: WASAPI.
//!
//! Enumeration/volume/mute via the MMDevice API + `IAudioEndpointVolume`;
//! default-endpoint switching via the undocumented `IPolicyConfig` interface.
//! Routing:
//!   1 enabled  -> that device becomes the Windows default render endpoint
//!                 (IPolicyConfig), no extra plumbing.
//!   2+ enabled -> primary device (kept/made default endpoint) is loopback-
//!                 captured and mirrored to every other enabled device by
//!                 the engine in `engine.rs`.
//!   0 enabled  -> engine stopped; default endpoint muted ("silent mode"),
//!                 unmuted again when leaving silent mode if we muted it.
//!
//! Threading: the backend is called from arbitrary Tauri threads and COM
//! callbacks arrive on MTA worker threads, so every entry point joins the
//! multithreaded apartment first (see [`ensure_com`]). All WASAPI / MMDevice
//! API objects are documented free-threaded, which is what makes the
//! `Agile` Send wrapper below sound.

mod engine;

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use log::{debug, info, warn};
use windows::core::{implement, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::{
    PKEY_Device_EnumeratorName, PKEY_Device_FriendlyName,
};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eConsole, eMultimedia, eRender, DigitalAudioDisplayDevice as FF_HDMI,
    Headphones as FF_HEADPHONES, Headset as FF_HEADSET, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    PKEY_AudioEndpoint_FormFactor, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
    DEVICE_STATE_ACTIVE, EDataFlow, ERole, SPDIF as FF_SPDIF,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use crate::{AudioBackend, BackendEvent, Device, DeviceType};

// ---------------------------------------------------------------------------
// COM lifecycle
// ---------------------------------------------------------------------------

struct ComGuard {
    uninit_on_drop: bool,
}

impl ComGuard {
    fn new() -> Self {
        // S_OK/S_FALSE: this thread joined the MTA and owes a CoUninitialize.
        // RPC_E_CHANGED_MODE: the thread is already an STA (e.g. a UI thread);
        // COM is usable there as-is, we just must not uninitialize it.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            uninit_on_drop: hr.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit_on_drop {
            unsafe { CoUninitialize() };
        }
    }
}

/// Initialize COM (MTA) once per thread, balanced on thread exit.
fn ensure_com() {
    thread_local! {
        static COM: ComGuard = ComGuard::new();
    }
    COM.with(|_| {});
}

/// Marks COM state as movable across threads. Sound because every WASAPI /
/// MMDevice API object is documented free-threaded and each thread that
/// touches them runs [`ensure_com`] first.
struct Agile<T>(T);
// SAFETY: see type-level comment.
unsafe impl<T> Send for Agile<T> {}

// ---------------------------------------------------------------------------
// Shared MMDevice helpers (also used by engine.rs)
// ---------------------------------------------------------------------------

fn create_enumerator() -> anyhow::Result<IMMDeviceEnumerator> {
    ensure_com();
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .context("create IMMDeviceEnumerator")
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Consume an OS-allocated PWSTR (CoTaskMemAlloc'd) into a String.
fn take_pwstr(s: PWSTR) -> anyhow::Result<String> {
    let text = unsafe { s.to_string() };
    unsafe { CoTaskMemFree(Some(s.as_ptr() as *const _)) };
    text.context("OS string is not valid UTF-16")
}

fn device_id(device: &IMMDevice) -> anyhow::Result<String> {
    take_pwstr(unsafe { device.GetId() }.context("IMMDevice::GetId")?)
}

fn get_device(enumerator: &IMMDeviceEnumerator, id: &str) -> anyhow::Result<IMMDevice> {
    let wide = to_wide(id);
    unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
        .with_context(|| format!("open endpoint {id}"))
}

fn endpoint_volume(device: &IMMDevice) -> anyhow::Result<IAudioEndpointVolume> {
    unsafe { device.Activate(CLSCTX_ALL, None) }.context("activate IAudioEndpointVolume")
}

/// All active render endpoints as (device, endpoint id) pairs.
fn enumerate_endpoints(
    enumerator: &IMMDeviceEnumerator,
) -> anyhow::Result<Vec<(IMMDevice, String)>> {
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .context("enumerate render endpoints")?;
    let count = unsafe { collection.GetCount() }.context("endpoint count")?;
    let mut endpoints = Vec::with_capacity(count as usize);
    for i in 0..count {
        let device = unsafe { collection.Item(i) }.with_context(|| format!("endpoint #{i}"))?;
        let id = device_id(&device)?;
        endpoints.push((device, id));
    }
    Ok(endpoints)
}

fn default_endpoint_id(enumerator: &IMMDeviceEnumerator) -> Option<String> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.ok()?;
    device_id(&device).ok()
}

// ---------------------------------------------------------------------------
// Property store helpers
// ---------------------------------------------------------------------------

fn prop_string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    let mut value = unsafe { store.GetValue(key) }.ok()?;
    let text = unsafe { PropVariantToStringAlloc(&value) }
        .ok()
        .and_then(|s| take_pwstr(s).ok())
        // VT_EMPTY converts to "" rather than failing; treat it as absent.
        .filter(|s| !s.is_empty());
    let _ = unsafe { PropVariantClear(&mut value) };
    text
}

fn prop_u32(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<u32> {
    let mut value = unsafe { store.GetValue(key) }.ok()?;
    let number = unsafe { PropVariantToUInt32(&value) }.ok();
    let _ = unsafe { PropVariantClear(&mut value) };
    number
}

/// Classify an endpoint. The transport (Bluetooth/USB, from the device
/// enumerator name) is checked around the form factor: BT headsets report the
/// Headphones/Headset form factor, so Bluetooth must win first (parity with
/// the Python implementation's priority order), while USB is only a fallback
/// for endpoints whose form factor carries no more specific information.
fn infer_device_type(name: &str, form_factor: Option<u32>, bus: Option<&str>) -> DeviceType {
    let lower = name.to_ascii_lowercase();
    if bus.is_some_and(|b| b.eq_ignore_ascii_case("BTHENUM"))
        || lower.contains("bluetooth")
        || lower.contains("bt-")
    {
        return DeviceType::Bluetooth;
    }
    match form_factor {
        Some(f) if f == FF_HEADPHONES.0 as u32 || f == FF_HEADSET.0 as u32 => {
            return DeviceType::Headphones
        }
        Some(f) if f == FF_HDMI.0 as u32 => return DeviceType::Hdmi,
        Some(f) if f == FF_SPDIF.0 as u32 => return DeviceType::Digital,
        _ => {}
    }
    if bus.is_some_and(|b| b.eq_ignore_ascii_case("USB")) || lower.contains("usb") {
        return DeviceType::Usb;
    }
    DeviceType::Speakers
}

fn read_device(device: &IMMDevice, id: &str, enabled: &[String]) -> anyhow::Result<Device> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.context("open property store")?;
    let name = prop_string(&store, &PKEY_Device_FriendlyName).unwrap_or_else(|| id.to_string());
    let form_factor = prop_u32(&store, &PKEY_AudioEndpoint_FormFactor);
    let bus = prop_string(&store, &PKEY_Device_EnumeratorName);
    let volume = endpoint_volume(device)?;
    let level = unsafe { volume.GetMasterVolumeLevelScalar() }.context("get volume")?;
    let muted = unsafe { volume.GetMute() }.context("get mute")?.as_bool();
    Ok(Device {
        device_type: infer_device_type(&name, form_factor, bus.as_deref()),
        id: id.to_string(),
        name,
        enabled: enabled.iter().any(|e| e == id),
        volume: level.clamp(0.0, 1.0),
        muted,
    })
}

// ---------------------------------------------------------------------------
// Default-endpoint switching (IPolicyConfig)
// ---------------------------------------------------------------------------

/// Event context passed on our own volume/mute writes so the endpoint volume
/// callback can tell our changes apart from external ones (loop prevention).
const APP_EVENT_CONTEXT: GUID = GUID::from_u128(0x9a176f4e_2b3d_4c8a_9e5f_0d7c1b2a3e4d);

#[allow(dead_code, non_camel_case_types)]
mod policy_config {
    use core::ffi::c_void;
    use windows::core::{interface, IUnknown, IUnknown_Vtbl, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::ERole;

    /// `IPolicyConfig` is the undocumented-but-ubiquitous interface behind the
    /// "set default device" button of the Sound control panel (stable since
    /// Vista; used by every audio switcher in existence). Only
    /// `set_default_endpoint` (vtable slot 11) is ever called; the preceding
    /// methods are declared solely to keep the vtable slot layout correct,
    /// with their pointer parameters erased to `*mut c_void`.
    #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        pub fn get_mix_format(&self, device_id: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        pub fn get_device_format(
            &self,
            device_id: PCWSTR,
            default: i32,
            format: *mut *mut c_void,
        ) -> HRESULT;
        pub fn reset_device_format(&self, device_id: PCWSTR) -> HRESULT;
        pub fn set_device_format(
            &self,
            device_id: PCWSTR,
            endpoint_format: *mut c_void,
            mix_format: *mut c_void,
        ) -> HRESULT;
        pub fn get_processing_period(
            &self,
            device_id: PCWSTR,
            default: i32,
            default_period: *mut i64,
            min_period: *mut i64,
        ) -> HRESULT;
        pub fn set_processing_period(&self, device_id: PCWSTR, period: *mut i64) -> HRESULT;
        pub fn get_share_mode(&self, device_id: PCWSTR, mode: *mut c_void) -> HRESULT;
        pub fn set_share_mode(&self, device_id: PCWSTR, mode: *mut c_void) -> HRESULT;
        pub fn get_property_value(
            &self,
            device_id: PCWSTR,
            fx_store: i32,
            key: *const c_void,
            value: *mut c_void,
        ) -> HRESULT;
        pub fn set_property_value(
            &self,
            device_id: PCWSTR,
            fx_store: i32,
            key: *const c_void,
            value: *mut c_void,
        ) -> HRESULT;
        pub fn set_default_endpoint(&self, device_id: PCWSTR, role: ERole) -> HRESULT;
        pub fn set_endpoint_visibility(&self, device_id: PCWSTR, visible: i32) -> HRESULT;
    }
}

use policy_config::IPolicyConfig;

const POLICY_CONFIG_CLSID: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

fn set_default_endpoint(id: &str) -> anyhow::Result<()> {
    ensure_com();
    let policy: IPolicyConfig = unsafe { CoCreateInstance(&POLICY_CONFIG_CLSID, None, CLSCTX_ALL) }
        .context("create PolicyConfig client")?;
    let wide = to_wide(id);
    for role in [eConsole, eMultimedia] {
        unsafe { policy.set_default_endpoint(PCWSTR(wide.as_ptr()), role) }
            .ok()
            .with_context(|| format!("SetDefaultEndpoint({id}, role {})", role.0))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Monitoring (device + volume change notifications)
// ---------------------------------------------------------------------------

#[implement(IMMNotificationClient)]
struct DeviceNotifications {
    tx: Sender<BackendEvent>,
}

impl DeviceNotifications {
    fn changed(&self) {
        let _ = self.tx.send(BackendEvent::DevicesChanged);
    }
}

impl IMMNotificationClient_Impl for DeviceNotifications_Impl {
    fn OnDeviceStateChanged(
        &self,
        _device_id: &PCWSTR,
        _new_state: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        self.changed();
        Ok(())
    }

    fn OnDeviceAdded(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        self.changed();
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        self.changed();
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _device_id: &PCWSTR,
    ) -> windows::core::Result<()> {
        // Fires once per role; forward one event (coalescing happens upstream,
        // this just cuts the obvious duplicates).
        if flow == eRender && role == eConsole {
            self.changed();
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _device_id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeNotifications {
    tx: Sender<BackendEvent>,
    id: String,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeNotifications_Impl {
    fn OnNotify(
        &self,
        data: *mut AUDIO_VOLUME_NOTIFICATION_DATA,
    ) -> windows::core::Result<()> {
        let Some(data) = (unsafe { data.as_ref() }) else {
            return Ok(());
        };
        if data.guidEventContext == APP_EVENT_CONTEXT {
            return Ok(()); // our own write echoing back
        }
        // The notification does not say *what* changed; report both and let
        // the app reconcile against its last known state.
        let _ = self.tx.send(BackendEvent::VolumeChanged {
            id: self.id.clone(),
            volume: data.fMasterVolume.clamp(0.0, 1.0),
        });
        let _ = self.tx.send(BackendEvent::MuteChanged {
            id: self.id.clone(),
            muted: data.bMuted.as_bool(),
        });
        Ok(())
    }
}

struct VolumeWatch {
    id: String,
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

struct Monitor {
    enumerator: IMMDeviceEnumerator,
    notifications: IMMNotificationClient,
    volume_watches: Vec<VolumeWatch>,
    tx: Sender<BackendEvent>,
}

impl Monitor {
    fn start(tx: Sender<BackendEvent>) -> anyhow::Result<Self> {
        let enumerator = create_enumerator()?;
        let notifications: IMMNotificationClient = DeviceNotifications { tx: tx.clone() }.into();
        unsafe { enumerator.RegisterEndpointNotificationCallback(&notifications) }
            .context("register device notification callback")?;
        let mut monitor = Self {
            enumerator,
            notifications,
            volume_watches: Vec::new(),
            tx,
        };
        // On Err the drop of `monitor` unregisters the endpoint callback
        // registered above (see the Drop impl below).
        monitor.refresh_volume_watches()?;
        Ok(monitor)
    }

    /// (Re)register an IAudioEndpointVolumeCallback on every active endpoint.
    /// Called after every enumeration so watches follow device hotplug.
    fn refresh_volume_watches(&mut self) -> anyhow::Result<()> {
        self.clear_volume_watches();
        for (device, id) in enumerate_endpoints(&self.enumerator)? {
            let volume = match endpoint_volume(&device) {
                Ok(v) => v,
                Err(e) => {
                    warn!("cannot watch volume of {id}: {e:#}");
                    continue;
                }
            };
            let callback: IAudioEndpointVolumeCallback = VolumeNotifications {
                tx: self.tx.clone(),
                id: id.clone(),
            }
            .into();
            if let Err(e) = unsafe { volume.RegisterControlChangeNotify(&callback) } {
                warn!("volume callback registration failed for {id}: {e}");
                continue;
            }
            self.volume_watches.push(VolumeWatch {
                id,
                volume,
                callback,
            });
        }
        Ok(())
    }

    fn clear_volume_watches(&mut self) {
        for watch in self.volume_watches.drain(..) {
            if let Err(e) = unsafe { watch.volume.UnregisterControlChangeNotify(&watch.callback) }
            {
                debug!("unregister volume callback for {}: {e}", watch.id);
            }
        }
    }

}

impl Drop for Monitor {
    fn drop(&mut self) {
        // Teardown lives in Drop so every exit — cleanup(), a monitor being
        // replaced by start_monitoring, or an error path inside
        // Monitor::start itself — unregisters the COM callbacks; a leaked
        // registration would keep firing ghost events forever. May run on
        // any thread (the objects are free-threaded, see `Agile`).
        ensure_com();
        self.clear_volume_watches();
        if let Err(e) = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.notifications)
        } {
            debug!("unregister device notification callback: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Minimum spacing between automatic engine rebuilds after stream failures,
/// so a persistently failing device cannot drive a restart loop.
const ENGINE_RESTART_COOLDOWN: Duration = Duration::from_secs(2);

pub struct WindowsBackend {
    /// Currently applied enabled set (endpoint IDs).
    enabled: Vec<String>,
    engine: Option<engine::Engine>,
    /// The applied set is empty ("silent mode"): the current default endpoint
    /// is kept muted until routing changes, re-enforced after failed mutes
    /// and across default-endpoint changes (see `enforce_silent_mode`).
    desired_silent: bool,
    /// Endpoint we muted when entering silent mode; `None` also when the user
    /// had already muted it themselves (then the mute is not ours to undo).
    silent_muted: Option<String>,
    /// When the engine was last rebuilt automatically after a stream failure.
    last_engine_restart: Option<Instant>,
    monitor: Option<Agile<Monitor>>,
    tx: Option<Sender<BackendEvent>>,
}

impl WindowsBackend {
    pub fn new() -> anyhow::Result<Self> {
        ensure_com();
        let enumerator = create_enumerator()?;
        // Unlike Linux there is no persistent plumbing a crashed run could
        // leave behind (the loopback engine dies with the process); the one
        // irrecoverable leftover would be a still-muted default endpoint,
        // which is indistinguishable from a user mute and therefore left alone.
        let enabled = match default_endpoint_id(&enumerator) {
            Some(id) => vec![id],
            None => {
                warn!("no default render endpoint; starting with empty enabled set");
                Vec::new()
            }
        };
        info!("windows audio backend ready; initially enabled: {enabled:?}");
        Ok(Self {
            enabled,
            engine: None,
            desired_silent: false,
            silent_muted: None,
            last_engine_restart: None,
            monitor: None,
            tx: None,
        })
    }

    fn emit_error(&self, message: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BackendEvent::Error(message));
        }
    }

    fn stop_engine(&mut self) {
        self.engine = None; // Engine::drop stops and joins the threads
    }

    fn make_default(&mut self, id: &str) {
        if let Err(e) = set_default_endpoint(id) {
            warn!("could not set default endpoint {id}: {e:#}");
            self.emit_error(format!("Failed to set default output device: {e}"));
        }
    }

    /// Mute the current default endpoint, remembering it only if the mute is
    /// ours (i.e. it was not already muted by the user).
    fn enter_silent_mode(&mut self, enumerator: &IMMDeviceEnumerator) -> anyhow::Result<()> {
        let device = match unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) } {
            Ok(d) => d,
            Err(e) => {
                // No render endpoints at all: the system is silent already.
                debug!("silent mode: no default endpoint to mute ({e})");
                return Ok(());
            }
        };
        let id = device_id(&device)?;
        let volume = endpoint_volume(&device)?;
        let was_muted = unsafe { volume.GetMute() }.context("get mute")?.as_bool();
        if was_muted {
            debug!("default endpoint {id} already muted; not taking ownership");
            return Ok(());
        }
        unsafe { volume.SetMute(true, &APP_EVENT_CONTEXT) }.context("mute default endpoint")?;
        info!("silent mode: muted default endpoint {id}");
        self.silent_muted = Some(id);
        Ok(())
    }

    /// Bring silent mode in line with the CURRENT default endpoint. Keyed off
    /// present state rather than transitions, so a previously failed mute is
    /// retried and the mute follows the default endpoint when Windows moves
    /// it while silent (e.g. a hotplugged device auto-promoted to default).
    /// A no-op once our mute is in place on the current default.
    fn enforce_silent_mode(&mut self, enumerator: &IMMDeviceEnumerator) -> anyhow::Result<()> {
        if self.silent_muted.is_some() && self.silent_muted != default_endpoint_id(enumerator) {
            // The default moved out from under our mute: release the old
            // endpoint, then claim the new one below.
            self.leave_silent_mode(enumerator);
        }
        if self.silent_muted.is_none() {
            self.enter_silent_mode(enumerator)?;
        }
        Ok(())
    }

    /// Self-heal from the events our own failure paths emit. The event pump
    /// re-lists devices on every `DevicesChanged` — including the one a dying
    /// engine stream sends (see `engine.rs`) and the one a default-endpoint
    /// change produces — so `list_devices` is where a degraded engine gets
    /// rebuilt against the surviving devices and where silent mode is
    /// re-enforced.
    fn reconcile_routing(&mut self, enumerator: &IMMDeviceEnumerator) {
        if self.engine.as_ref().is_some_and(|e| !e.is_running()) {
            let cooling_down = self
                .last_engine_restart
                .is_some_and(|t| t.elapsed() < ENGINE_RESTART_COOLDOWN);
            if cooling_down {
                debug!("engine stream died again within cooldown; deferring rebuild");
            } else {
                self.last_engine_restart = Some(Instant::now());
                info!("engine stream died; re-applying routing to surviving devices");
                let ids = self.enabled.clone();
                self.stop_engine();
                if let Err(e) = self.apply_enabled(&ids) {
                    warn!("engine rebuild after stream failure failed: {e:#}");
                    self.emit_error(format!("Could not restore multi-output routing: {e}"));
                }
            }
        }
        if self.desired_silent {
            if let Err(e) = self.enforce_silent_mode(enumerator) {
                warn!("silent mode re-enforcement failed: {e:#}");
            }
        }
    }

    /// Undo our silent-mode mute, if any. Degrades to a log entry when the
    /// device has meanwhile disappeared.
    fn leave_silent_mode(&mut self, enumerator: &IMMDeviceEnumerator) {
        let Some(id) = self.silent_muted.take() else {
            return;
        };
        let unmute = (|| -> anyhow::Result<()> {
            let device = get_device(enumerator, &id)?;
            let volume = endpoint_volume(&device)?;
            unsafe { volume.SetMute(false, &APP_EVENT_CONTEXT) }.context("unmute")?;
            Ok(())
        })();
        match unmute {
            Ok(()) => info!("silent mode left: unmuted {id}"),
            Err(e) => warn!("could not unmute {id} when leaving silent mode: {e:#}"),
        }
    }
}

impl AudioBackend for WindowsBackend {
    fn list_devices(&mut self) -> anyhow::Result<Vec<Device>> {
        ensure_com();
        let enumerator = create_enumerator()?;
        // Reconcile before reading, so the returned state reflects the
        // healed routing (rebuilt engine, re-enforced silent-mode mute).
        self.reconcile_routing(&enumerator);
        let endpoints = enumerate_endpoints(&enumerator)?;
        let mut devices = Vec::with_capacity(endpoints.len());
        for (device, id) in &endpoints {
            match read_device(device, id, &self.enabled) {
                Ok(d) => devices.push(d),
                Err(e) => warn!("skipping endpoint {id}: {e:#}"),
            }
        }
        // Volume watches follow the device list: on DevicesChanged the app
        // re-lists devices, which is exactly when registrations must be
        // refreshed to cover hotplugged endpoints.
        if let Some(monitor) = &mut self.monitor {
            if let Err(e) = monitor.0.refresh_volume_watches() {
                debug!("volume watch refresh failed: {e:#}");
            }
        }
        Ok(devices)
    }

    fn apply_enabled(&mut self, ids: &[String]) -> anyhow::Result<()> {
        ensure_com();
        let enumerator = create_enumerator()?;
        let active = enumerate_endpoints(&enumerator)?;
        // Ids that no longer exist are ignored per the trait contract.
        let ids: Vec<String> = ids
            .iter()
            .filter(|id| active.iter().any(|(_, a)| a == *id))
            .cloned()
            .collect();
        self.enabled = ids.clone();
        // Recorded before attempting the mute, so a failed mute is retried
        // by later applies and by the monitor-driven reconcile path.
        self.desired_silent = ids.is_empty();

        match ids.len() {
            0 => {
                self.stop_engine();
                self.enforce_silent_mode(&enumerator)?;
            }
            1 => {
                self.stop_engine();
                self.leave_silent_mode(&enumerator);
                self.make_default(&ids[0]);
            }
            _ => {
                self.leave_silent_mode(&enumerator);
                // Keep the current default as primary when it stays enabled;
                // it is the device the user hears now, so fan-out from it.
                let primary = match default_endpoint_id(&enumerator) {
                    Some(d) if ids.contains(&d) => d,
                    _ => ids[0].clone(),
                };
                let secondaries: Vec<String> =
                    ids.iter().filter(|i| **i != primary).cloned().collect();
                self.make_default(&primary);
                let unchanged = self.engine.as_ref().is_some_and(|e| {
                    e.is_running() && e.primary() == primary && e.secondaries() == secondaries
                });
                if unchanged {
                    debug!("engine already running with identical routing; not restarting");
                } else {
                    self.stop_engine();
                    let engine =
                        engine::Engine::start(primary, secondaries, self.tx.clone())
                            .context("start loopback fan-out engine")?;
                    self.engine = Some(engine);
                }
            }
        }
        Ok(())
    }

    fn set_volume(&mut self, id: &str, volume: f32) -> anyhow::Result<()> {
        ensure_com();
        let enumerator = create_enumerator()?;
        let device = get_device(&enumerator, id)?;
        let endpoint = endpoint_volume(&device)?;
        unsafe { endpoint.SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), &APP_EVENT_CONTEXT) }
            .with_context(|| format!("set volume of {id}"))
    }

    fn set_muted(&mut self, id: &str, muted: bool) -> anyhow::Result<()> {
        ensure_com();
        let enumerator = create_enumerator()?;
        let device = get_device(&enumerator, id)?;
        let endpoint = endpoint_volume(&device)?;
        // A user-driven unmute of the silent-mode endpoint takes the mute
        // ownership away from us — and stops silent-mode re-enforcement
        // (until the next apply), so we do not fight the user's choice.
        if !muted && self.silent_muted.as_deref() == Some(id) {
            self.silent_muted = None;
            self.desired_silent = false;
        }
        unsafe { endpoint.SetMute(muted, &APP_EVENT_CONTEXT) }
            .with_context(|| format!("set mute of {id}"))
    }

    fn start_monitoring(&mut self, tx: Sender<BackendEvent>) -> anyhow::Result<()> {
        ensure_com();
        // May be called repeatedly: dropping any previous monitor unregisters
        // its callbacks before the replacement registers new ones.
        if self.monitor.take().is_some() {
            debug!("monitoring already running; replacing the monitor");
        }
        self.tx = None;
        let monitor = Monitor::start(tx.clone())?;
        self.monitor = Some(Agile(monitor));
        self.tx = Some(tx);
        info!("windows audio monitoring started");
        Ok(())
    }

    fn cleanup(&mut self) -> anyhow::Result<()> {
        ensure_com();
        self.stop_engine();
        // The primary already is the plain default endpoint, which satisfies
        // "leave the system on a sane default" without restoring history.
        self.desired_silent = false;
        if let Ok(enumerator) = create_enumerator() {
            self.leave_silent_mode(&enumerator);
        }
        self.monitor = None; // Monitor::drop unregisters its callbacks
        self.tx = None;
        info!("windows audio backend cleaned up");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_bus_beats_headphones_form_factor() {
        // BT headsets report the Headphones/Headset form factor; the
        // transport must win (see infer_device_type's priority order).
        assert_eq!(
            infer_device_type("WH-1000XM5", Some(FF_HEADPHONES.0 as u32), Some("BTHENUM")),
            DeviceType::Bluetooth
        );
        assert_eq!(
            infer_device_type("Buds Pro", Some(FF_HEADSET.0 as u32), Some("bthenum")),
            DeviceType::Bluetooth
        );
    }

    #[test]
    fn bluetooth_name_keywords_win_without_bus_info() {
        assert_eq!(
            infer_device_type("Bluetooth Speaker", Some(FF_HDMI.0 as u32), None),
            DeviceType::Bluetooth
        );
        assert_eq!(
            infer_device_type("BT-900 Stereo", None, None),
            DeviceType::Bluetooth
        );
    }

    #[test]
    fn headphones_and_headset_form_factors_map_to_headphones() {
        assert_eq!(
            infer_device_type("Realtek Audio", Some(FF_HEADPHONES.0 as u32), None),
            DeviceType::Headphones
        );
        assert_eq!(
            infer_device_type("Realtek Audio", Some(FF_HEADSET.0 as u32), None),
            DeviceType::Headphones
        );
    }

    #[test]
    fn headphones_form_factor_beats_usb_transport() {
        // A USB headset is headphones to the user; USB is only a fallback
        // for endpoints whose form factor says nothing more specific.
        assert_eq!(
            infer_device_type("USB Gaming Headset", Some(FF_HEADSET.0 as u32), Some("USB")),
            DeviceType::Headphones
        );
    }

    #[test]
    fn hdmi_and_spdif_form_factors_map_to_hdmi_and_digital() {
        assert_eq!(
            infer_device_type("LG TV", Some(FF_HDMI.0 as u32), None),
            DeviceType::Hdmi
        );
        assert_eq!(
            infer_device_type("Optical Out", Some(FF_SPDIF.0 as u32), None),
            DeviceType::Digital
        );
    }

    #[test]
    fn usb_bus_or_name_keyword_is_the_fallback_transport() {
        assert_eq!(
            infer_device_type("Scarlett 2i2", None, Some("USB")),
            DeviceType::Usb
        );
        assert_eq!(
            infer_device_type("Scarlett 2i2", None, Some("usb")),
            DeviceType::Usb
        );
        assert_eq!(
            infer_device_type("Generic USB DAC", None, None),
            DeviceType::Usb
        );
    }

    #[test]
    fn unknown_everything_defaults_to_speakers() {
        assert_eq!(
            infer_device_type("Realtek High Definition Audio", None, None),
            DeviceType::Speakers
        );
        // An unrecognized form factor on a non-USB/BT bus falls through too.
        assert_eq!(
            infer_device_type("Mystery Endpoint", Some(9999), Some("HDAUDIO")),
            DeviceType::Speakers
        );
    }
}
