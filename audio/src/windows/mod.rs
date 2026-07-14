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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
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

/// Per-thread COM initialization, balanced on thread exit (see `ensure_com`).
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

/// Adapter interface friendly name (`PKEY_DeviceInterface_FriendlyName`,
/// e.g. "Intel® Smart Sound Technology for Bluetooth® Audio"): audio-offload
/// stacks route Bluetooth endpoints through the platform DSP driver, so the
/// endpoint's own enumerator says INTELAUDIO/ACX rather than BTHENUM and the
/// adapter name is the only documented property still naming the transport.
const PKEY_ADAPTER_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x026e516e_b814_414b_83cd_856d6fef4822),
    pid: 2,
};

/// Undocumented but long-stable MMDevice endpoint property holding the PnP
/// path of the audio adapter *as seen by the bus* — for Bluetooth endpoints a
/// `BTHENUM\{service-uuid}_...` path even on offload stacks whose enumerator
/// name no longer says BTHENUM. The service UUID distinguishes A2DP
/// (`0000110b`, AudioSink) from Hands-Free telephony (`0000111e`/`0000111f`).
const PKEY_ENDPOINT_BUS_PATH: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb3f8fa53_0004_438e_9003_51a46e139bfc),
    pid: 39,
};

/// Adapter friendly name under the same undocumented MMDevice set; present
/// on endpoints (observed: Bluetooth offload) where the documented
/// [`PKEY_ADAPTER_NAME`] is empty.
const PKEY_ENDPOINT_ADAPTER_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb3f8fa53_0004_438e_9003_51a46e139bfc),
    pid: 6,
};

/// Transport-identifying endpoint properties, gathered once per endpoint and
/// consumed by [`infer_device_type`] and [`is_handsfree_endpoint`].
#[derive(Default)]
struct TransportInfo {
    /// `PKEY_Device_EnumeratorName`: BTHENUM/BTHHFENUM on the classic
    /// Bluetooth stack; the platform audio driver (e.g. INTELAUDIO) on
    /// offload stacks.
    enumerator: Option<String>,
    /// See [`PKEY_ENDPOINT_BUS_PATH`].
    bus_path: Option<String>,
    /// Adapter name from [`PKEY_ADAPTER_NAME`], falling back to
    /// [`PKEY_ENDPOINT_ADAPTER_NAME`].
    adapter: Option<String>,
}

impl TransportInfo {
    fn read(store: &IPropertyStore) -> Self {
        Self {
            enumerator: prop_string(store, &PKEY_Device_EnumeratorName),
            bus_path: prop_string(store, &PKEY_ENDPOINT_BUS_PATH),
            adapter: prop_string(store, &PKEY_ADAPTER_NAME)
                .or_else(|| prop_string(store, &PKEY_ENDPOINT_ADAPTER_NAME)),
        }
    }

    fn is_bluetooth(&self) -> bool {
        let enum_bt = self
            .enumerator
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case("BTHENUM") || e.eq_ignore_ascii_case("BTHHFENUM"));
        let path_bt = self
            .bus_path
            .as_deref()
            .is_some_and(|p| p.to_ascii_lowercase().contains("bthenum"));
        let adapter_bt = self
            .adapter
            .as_deref()
            .is_some_and(|a| a.to_ascii_lowercase().contains("bluetooth"));
        enum_bt || path_bt || adapter_bt
    }

    fn is_usb(&self) -> bool {
        let enum_usb = self
            .enumerator
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case("USB"));
        let adapter_usb = self
            .adapter
            .as_deref()
            .is_some_and(|a| a.to_ascii_lowercase().contains("usb"));
        enum_usb || adapter_usb
    }
}

/// Classify an endpoint. The transport (Bluetooth/USB) is checked around the
/// form factor: BT headsets report the Headphones/Headset form factor, so
/// Bluetooth must win first (priority order is part of the cross-platform
/// contract, see [`DeviceType`]), while USB is only a fallback for endpoints
/// whose form factor carries no more specific information. Real hardware
/// (Intel SST offload) showed the enumerator name alone is not enough, hence
/// the layered [`TransportInfo`] checks.
fn infer_device_type(name: &str, form_factor: Option<u32>, transport: &TransportInfo) -> DeviceType {
    let lower = name.to_ascii_lowercase();
    if transport.is_bluetooth() || lower.contains("bluetooth") || lower.contains("bt-") {
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
    if transport.is_usb() || lower.contains("usb") {
        return DeviceType::Usb;
    }
    DeviceType::Speakers
}

/// Bluetooth Hands-Free (HFP) telephony endpoints are excluded from the
/// device list: they coexist with the same headset's A2DP endpoint, and
/// opening a render stream on one flips the headset out of A2DP — enabling
/// both (e.g. via "Select all") collapses the headset to telephony-quality
/// audio and invalidates the A2DP stream. One row per physical device also
/// matches the Linux backend, where HFP is a profile, not a separate sink.
fn is_handsfree_endpoint(name: &str, form_factor: Option<u32>, transport: &TransportInfo) -> bool {
    // Classic stack: HFP endpoints enumerate under BTHHFENUM.
    if transport
        .enumerator
        .as_deref()
        .is_some_and(|e| e.eq_ignore_ascii_case("BTHHFENUM"))
    {
        return true;
    }
    // Offload stacks: the bus path names the Bluetooth service —
    // 0000111e = Hands-Free, 0000111f = Hands-Free Audio Gateway.
    if transport.bus_path.as_deref().is_some_and(|p| {
        let p = p.to_ascii_lowercase();
        p.contains("0000111e") || p.contains("0000111f")
    }) {
        return true;
    }
    // Last resort: the driver-provided endpoint name. Only trusted together
    // with the Headset form factor so a coincidentally named wired device
    // cannot be hidden.
    form_factor == Some(FF_HEADSET.0 as u32) && name.to_ascii_lowercase().contains("hands-free")
}

fn read_device(device: &IMMDevice, id: &str, enabled: &[String]) -> anyhow::Result<Device> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.context("open property store")?;
    let name = prop_string(&store, &PKEY_Device_FriendlyName).unwrap_or_else(|| id.to_string());
    let form_factor = prop_u32(&store, &PKEY_AudioEndpoint_FormFactor);
    let transport = TransportInfo::read(&store);
    let volume = endpoint_volume(device)?;
    let level = unsafe { volume.GetMasterVolumeLevelScalar() }.context("get volume")?;
    let muted = unsafe { volume.GetMute() }.context("get mute")?.as_bool();
    Ok(Device {
        device_type: infer_device_type(&name, form_factor, &transport),
        id: id.to_string(),
        name,
        enabled: enabled.iter().any(|e| e == id),
        volume: level.clamp(0.0, 1.0),
        muted,
    })
}

/// Is this endpoint a Hands-Free telephony endpoint (see
/// [`is_handsfree_endpoint`])? Property-read failures count as "no": better
/// to show a questionable row than to hide a real device.
fn device_is_handsfree(device: &IMMDevice, id: &str) -> bool {
    let Ok(store) = (unsafe { device.OpenPropertyStore(STGM_READ) }) else {
        return false;
    };
    let name = prop_string(&store, &PKEY_Device_FriendlyName).unwrap_or_else(|| id.to_string());
    let form_factor = prop_u32(&store, &PKEY_AudioEndpoint_FormFactor);
    is_handsfree_endpoint(&name, form_factor, &TransportInfo::read(&store))
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
    // Both roles ordinary apps follow; eCommunications is deliberately left
    // alone so voice apps keep their chosen device.
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

    /// Bring the IAudioEndpointVolumeCallback registrations in line with the
    /// active endpoints. Called after every enumeration so watches follow
    /// device hotplug. Incremental on purpose: existing registrations are
    /// kept, not torn down and re-created — a change notification landing in
    /// an unregister/re-register window would be silently lost (the UI would
    /// show a stale volume until some unrelated event), and enumeration runs
    /// on every pump cycle.
    fn refresh_volume_watches(&mut self) -> anyhow::Result<()> {
        let endpoints = enumerate_endpoints(&self.enumerator)?;
        // Drop watches whose endpoint is gone.
        let (keep, drop): (Vec<VolumeWatch>, Vec<VolumeWatch>) = self
            .volume_watches
            .drain(..)
            .partition(|w| endpoints.iter().any(|(_, id)| *id == w.id));
        self.volume_watches = keep;
        for watch in drop {
            if let Err(e) = unsafe { watch.volume.UnregisterControlChangeNotify(&watch.callback) }
            {
                debug!("unregister volume callback for {}: {e}", watch.id);
            }
        }
        // Watch endpoints that appeared.
        for (device, id) in endpoints {
            if self.volume_watches.iter().any(|w| w.id == id) {
                continue;
            }
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

/// WASAPI backend; see the module docs for the routing scheme and COM
/// threading rules. Unit-tested, and verified on real hardware by the
/// opt-in live E2E in `tests/windows_live.rs` (see the crate docs on
/// platform coverage).
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
    /// A cooldown-deferred rebuild has a wake-up thread pending. Without the
    /// nudge, a failure landing inside the cooldown would consume its only
    /// DevicesChanged event and the engine would stay dead until some
    /// unrelated event re-listed devices.
    restart_nudge_pending: Arc<AtomicBool>,
    monitor: Option<Agile<Monitor>>,
    tx: Option<Sender<BackendEvent>>,
}

impl WindowsBackend {
    /// Backend seeded with the current default render endpoint as the
    /// initially enabled device.
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
            restart_nudge_pending: Arc::new(AtomicBool::new(false)),
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
                self.schedule_restart_nudge();
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
        } else if let Some(engine) = &self.engine {
            // Windows moved the default endpoint under a healthy fan-out
            // (user via Sound settings, or a hotplug auto-promotion). If the
            // new default is one of the enabled devices, adopt it as primary
            // so the mirrored source is the endpoint actually receiving
            // system audio. A default outside the enabled set is the user
            // deliberately routing around the app — leave it alone.
            let default = default_endpoint_id(enumerator);
            if let Some(default) = default {
                if default != engine.primary() && self.enabled.contains(&default) {
                    info!("default endpoint moved to enabled device {default}; re-anchoring fan-out");
                    let ids = self.enabled.clone();
                    if let Err(e) = self.apply_enabled(&ids) {
                        warn!("re-anchoring fan-out on new default failed: {e:#}");
                    }
                }
            }
        }
        if self.desired_silent {
            if let Err(e) = self.enforce_silent_mode(enumerator) {
                warn!("silent mode re-enforcement failed: {e:#}");
            }
        }
    }

    /// Arrange for a DevicesChanged nudge shortly after the restart cooldown
    /// expires, so a rebuild deferred by the cooldown is actually retried.
    /// The dying stream fires its DevicesChanged exactly once; when that
    /// event lands inside the cooldown, nothing else would ever re-run
    /// `reconcile_routing`. At most one nudge is pending at a time.
    fn schedule_restart_nudge(&self) {
        let Some(tx) = self.tx.clone() else {
            return; // no pump listening; nothing could react anyway
        };
        if self.restart_nudge_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let pending = Arc::clone(&self.restart_nudge_pending);
        let wait = self
            .last_engine_restart
            .map(|t| ENGINE_RESTART_COOLDOWN.saturating_sub(t.elapsed()))
            .unwrap_or(ENGINE_RESTART_COOLDOWN)
            + Duration::from_millis(100);
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            pending.store(false, Ordering::SeqCst);
            let _ = tx.send(BackendEvent::DevicesChanged);
        });
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
            if device_is_handsfree(device, id) {
                debug!("hiding Hands-Free telephony endpoint {id}");
                continue;
            }
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
        // Ids that no longer exist are ignored per the trait contract;
        // Hands-Free telephony endpoints are never routed to (they are
        // hidden from the list, but a stale id could still name one).
        let ids: Vec<String> = ids
            .iter()
            .filter(|id| {
                active
                    .iter()
                    .any(|(device, a)| a == *id && !device_is_handsfree(device, a))
            })
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

    fn transport(
        enumerator: Option<&str>,
        bus_path: Option<&str>,
        adapter: Option<&str>,
    ) -> TransportInfo {
        TransportInfo {
            enumerator: enumerator.map(str::to_string),
            bus_path: bus_path.map(str::to_string),
            adapter: adapter.map(str::to_string),
        }
    }

    fn enum_only(enumerator: &str) -> TransportInfo {
        transport(Some(enumerator), None, None)
    }

    #[test]
    fn bluetooth_bus_beats_headphones_form_factor() {
        // BT headsets report the Headphones/Headset form factor; the
        // transport must win (see infer_device_type's priority order).
        assert_eq!(
            infer_device_type("WH-1000XM5", Some(FF_HEADPHONES.0 as u32), &enum_only("BTHENUM")),
            DeviceType::Bluetooth
        );
        assert_eq!(
            infer_device_type("Buds Pro", Some(FF_HEADSET.0 as u32), &enum_only("bthenum")),
            DeviceType::Bluetooth
        );
    }

    #[test]
    fn offloaded_bluetooth_is_detected_without_a_bthenum_enumerator() {
        // Real hardware (Intel SST offload): the endpoint's enumerator is the
        // platform audio driver, and only the adapter name / bus path still
        // say Bluetooth. Values below are verbatim from a WH-1000XM5.
        assert_eq!(
            infer_device_type(
                "Headphones (WH-1000XM5)",
                Some(FF_HEADPHONES.0 as u32),
                &transport(
                    Some("INTELAUDIO"),
                    None,
                    Some("Intel® Smart Sound Technology for Bluetooth® Audio"),
                ),
            ),
            DeviceType::Bluetooth
        );
        assert_eq!(
            infer_device_type(
                "Headphones (WH-1000XM5)",
                Some(FF_HEADPHONES.0 as u32),
                &transport(
                    Some("INTELAUDIO"),
                    Some("{1}.BTHENUM\\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0DF0\\7&105BD535&0&AC800ADBDAF3_C00000000"),
                    None,
                ),
            ),
            DeviceType::Bluetooth
        );
    }

    #[test]
    fn bluetooth_name_keywords_win_without_bus_info() {
        assert_eq!(
            infer_device_type("Bluetooth Speaker", Some(FF_HDMI.0 as u32), &transport(None, None, None)),
            DeviceType::Bluetooth
        );
        assert_eq!(
            infer_device_type("BT-900 Stereo", None, &transport(None, None, None)),
            DeviceType::Bluetooth
        );
    }

    #[test]
    fn headphones_and_headset_form_factors_map_to_headphones() {
        assert_eq!(
            infer_device_type("Realtek Audio", Some(FF_HEADPHONES.0 as u32), &transport(None, None, None)),
            DeviceType::Headphones
        );
        assert_eq!(
            infer_device_type("Realtek Audio", Some(FF_HEADSET.0 as u32), &transport(None, None, None)),
            DeviceType::Headphones
        );
    }

    #[test]
    fn headphones_form_factor_beats_usb_transport() {
        // A USB headset is headphones to the user; USB is only a fallback
        // for endpoints whose form factor says nothing more specific.
        assert_eq!(
            infer_device_type("USB Gaming Headset", Some(FF_HEADSET.0 as u32), &enum_only("USB")),
            DeviceType::Headphones
        );
    }

    #[test]
    fn hdmi_and_spdif_form_factors_map_to_hdmi_and_digital() {
        assert_eq!(
            infer_device_type("LG TV", Some(FF_HDMI.0 as u32), &transport(None, None, None)),
            DeviceType::Hdmi
        );
        assert_eq!(
            infer_device_type("Optical Out", Some(FF_SPDIF.0 as u32), &transport(None, None, None)),
            DeviceType::Digital
        );
    }

    #[test]
    fn usb_bus_adapter_or_name_keyword_is_the_fallback_transport() {
        assert_eq!(
            infer_device_type("Scarlett 2i2", None, &enum_only("USB")),
            DeviceType::Usb
        );
        assert_eq!(
            infer_device_type("Scarlett 2i2", None, &enum_only("usb")),
            DeviceType::Usb
        );
        assert_eq!(
            infer_device_type("Scarlett 2i2", None, &transport(None, None, Some("USB Audio Device"))),
            DeviceType::Usb
        );
        assert_eq!(
            infer_device_type("Generic USB DAC", None, &transport(None, None, None)),
            DeviceType::Usb
        );
    }

    #[test]
    fn unknown_everything_defaults_to_speakers() {
        assert_eq!(
            infer_device_type("Realtek High Definition Audio", None, &transport(None, None, None)),
            DeviceType::Speakers
        );
        // An unrecognized form factor on a non-USB/BT bus falls through too.
        assert_eq!(
            infer_device_type("Mystery Endpoint", Some(9999), &enum_only("HDAUDIO")),
            DeviceType::Speakers
        );
    }

    #[test]
    fn handsfree_detected_by_service_uuid_on_offload_stacks() {
        // Verbatim bus path of a WH-1000XM5 Hands-Free endpoint on Intel SST:
        // 0000111e is the Bluetooth Hands-Free service class.
        let hfp = transport(
            Some("INTELAUDIO"),
            Some("{1}.BTHENUM\\{0000111E-0000-1000-8000-00805F9B34FB}_HCIBYPASS_VID&0002054C_PID&0DF0\\7&105BD535&0&AC800ADBDAF3_C00000000"),
            Some("Intel® Smart Sound Technology for Bluetooth® Audio"),
        );
        assert!(is_handsfree_endpoint(
            "Headset (WH-1000XM5 Hands-Free)",
            Some(FF_HEADSET.0 as u32),
            &hfp
        ));
        // The same headset's A2DP endpoint (service 0000110b) must stay.
        let a2dp = transport(
            Some("INTELAUDIO"),
            Some("{1}.BTHENUM\\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0DF0\\7&105BD535&0&AC800ADBDAF3_C00000000"),
            Some("Intel® Smart Sound Technology for Bluetooth® Audio"),
        );
        assert!(!is_handsfree_endpoint(
            "Headphones (WH-1000XM5)",
            Some(FF_HEADPHONES.0 as u32),
            &a2dp
        ));
    }

    #[test]
    fn handsfree_detected_by_bthhfenum_on_the_classic_stack() {
        assert!(is_handsfree_endpoint(
            "Headset (WH-1000XM5 Hands-Free AG Audio)",
            Some(FF_HEADSET.0 as u32),
            &enum_only("BTHHFENUM")
        ));
        assert!(!is_handsfree_endpoint(
            "Headphones (WH-1000XM5 Stereo)",
            Some(FF_HEADPHONES.0 as u32),
            &enum_only("BTHENUM")
        ));
    }

    #[test]
    fn handsfree_name_fallback_requires_the_headset_form_factor() {
        let none = transport(None, None, None);
        assert!(is_handsfree_endpoint(
            "Headset (Buds Hands-Free)",
            Some(FF_HEADSET.0 as u32),
            &none
        ));
        // Name alone must not hide a device with a different form factor...
        assert!(!is_handsfree_endpoint(
            "Hands-Free Sounding Speakers",
            Some(1),
            &none
        ));
        // ...and an ordinary headset without HFP markers must stay visible.
        assert!(!is_handsfree_endpoint(
            "USB Gaming Headset",
            Some(FF_HEADSET.0 as u32),
            &none
        ));
    }
}
