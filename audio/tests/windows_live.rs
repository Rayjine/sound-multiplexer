//! Full end-to-end test of `WindowsBackend` against the REAL audio hardware
//! of this machine.
//!
//! Run with:
//!   cargo test -p sound-multiplexer-audio --test windows_live -- --ignored --nocapture
//!
//! The test mutates live audio state — the default render endpoint, endpoint
//! volumes and mutes — and restores every bit of it through a Drop guard, so
//! the machine ends up exactly as found even when an assertion panics
//! mid-test. All verification goes through the test's own MMDevice/WASAPI
//! plumbing (its own enumerator, endpoint-volume reads and `IPolicyConfig`
//! declaration), never through the backend under test.
//!
//! The fan-out check is fully automatic, no listening required: while the
//! engine mirrors the primary endpoint to a secondary, the test plays a sine
//! tone on the primary (the system default, exactly like a normal app) and
//! loopback-captures the SECONDARY endpoint. The tone can only reach the
//! secondary's shared-mode mix through the engine, so signal energy there
//! proves the whole capture → ring → render path on real devices.

#![cfg(windows)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use sound_multiplexer_audio::windows::WindowsBackend;
use sound_multiplexer_audio::{AudioBackend, BackendEvent, Device, DeviceType};

use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eMultimedia, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient,
    IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};

/// Windows applies endpoint switches asynchronously; poll this long.
const APPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// Budget for a COM change notification to reach the monitor channel.
const EVENT_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(50);

/// Tone duration; long enough to cover Bluetooth render-path spin-up.
const TONE_SECS: f32 = 4.0;
/// Loopback capture window on the secondary, inside the tone window.
const CAPTURE_SECS: f32 = 2.0;
/// Signal threshold for "the tone audibly reached this endpoint": the tone
/// renders at 0.8 amplitude with the primary at 25% volume, so even after
/// the secondary's own volume scaling the RMS lands well above this.
const RMS_THRESHOLD: f32 = 0.005;

// ---------------------------------------------------------------------------
// Independent WASAPI helpers (the backend's plumbing is private; the test
// drives the OS directly, which also makes the assertions genuinely external)
// ---------------------------------------------------------------------------

fn ensure_com() {
    // Balanced never: the test process exits right after. S_FALSE on re-init
    // in helper threads is fine and intentionally ignored.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn enumerator() -> IMMDeviceEnumerator {
    ensure_com();
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .expect("create IMMDeviceEnumerator")
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get_device(enumerator: &IMMDeviceEnumerator, id: &str) -> IMMDevice {
    let wide = to_wide(id);
    unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
        .unwrap_or_else(|e| panic!("open endpoint {id}: {e}"))
}

fn endpoint_volume(device: &IMMDevice) -> IAudioEndpointVolume {
    unsafe { device.Activate(CLSCTX_ALL, None) }.expect("activate IAudioEndpointVolume")
}

fn device_id(device: &IMMDevice) -> String {
    let pw = unsafe { device.GetId() }.expect("IMMDevice::GetId");
    let text = unsafe { pw.to_string() }.expect("endpoint id is valid UTF-16");
    unsafe { CoTaskMemFree(Some(pw.as_ptr() as *const _)) };
    text
}

fn active_endpoint_ids() -> Vec<String> {
    let enumerator = enumerator();
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .expect("enumerate render endpoints");
    let count = unsafe { collection.GetCount() }.expect("endpoint count");
    (0..count)
        .map(|i| device_id(&unsafe { collection.Item(i) }.expect("endpoint item")))
        .collect()
}

fn default_endpoint_id() -> Option<String> {
    let enumerator = enumerator();
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.ok()?;
    Some(device_id(&device))
}

fn read_volume_mute(id: &str) -> (f32, bool) {
    let enumerator = enumerator();
    let volume = endpoint_volume(&get_device(&enumerator, id));
    let level = unsafe { volume.GetMasterVolumeLevelScalar() }.expect("read volume");
    let muted = unsafe { volume.GetMute() }.expect("read mute").as_bool();
    (level, muted)
}

/// External (non-app) volume write: a null event context, exactly what the
/// system tray or another app produces. Must never panic (used in Drop).
fn write_volume_mute_lenient(id: &str, level: f32, muted: bool) {
    let result = (|| -> windows::core::Result<()> {
        let enumerator = enumerator();
        let wide = to_wide(id);
        let device = unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }?;
        let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        unsafe { volume.SetMasterVolumeLevelScalar(level, std::ptr::null()) }?;
        unsafe { volume.SetMute(muted, std::ptr::null()) }?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("restore: could not write volume/mute of {id}: {e}");
    }
}

/// The test's own copy of the undocumented-but-ubiquitous IPolicyConfig
/// (see windows/mod.rs); declared independently so default-endpoint restores
/// never rely on the code under test.
#[allow(dead_code, non_camel_case_types)]
mod policy_config {
    use core::ffi::c_void;
    use windows::core::{interface, IUnknown, IUnknown_Vtbl, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::ERole;

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

const POLICY_CONFIG_CLSID: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

fn set_default_endpoint_lenient(id: &str) {
    ensure_com();
    let result = (|| -> windows::core::Result<()> {
        let policy: policy_config::IPolicyConfig =
            unsafe { CoCreateInstance(&POLICY_CONFIG_CLSID, None, CLSCTX_ALL) }?;
        let wide = to_wide(id);
        for role in [eConsole, eMultimedia] {
            unsafe { policy.set_default_endpoint(PCWSTR(wide.as_ptr()), role) }.ok()?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("restore: could not set default endpoint {id}: {e}");
    }
}

// ---------------------------------------------------------------------------
// State guard
// ---------------------------------------------------------------------------

/// Snapshot of everything the test (or the backend under test) may touch,
/// restored on Drop — even when an assertion panics mid-test.
struct AudioStateGuard {
    default_id: Option<String>,
    endpoint_state: Vec<(String, f32, bool)>,
}

impl AudioStateGuard {
    fn capture() -> Self {
        let endpoint_state = active_endpoint_ids()
            .into_iter()
            .map(|id| {
                let (volume, muted) = read_volume_mute(&id);
                (id, volume, muted)
            })
            .collect();
        Self {
            default_id: default_endpoint_id(),
            endpoint_state,
        }
    }
}

impl Drop for AudioStateGuard {
    fn drop(&mut self) {
        eprintln!("restoring pre-test audio state...");
        // Default endpoint first: volume restores must not race a mute the
        // backend may still be enforcing on a soon-to-be-non-default device.
        if let Some(id) = &self.default_id {
            set_default_endpoint_lenient(id);
        }
        let still_active = active_endpoint_ids();
        for (id, volume, muted) in &self.endpoint_state {
            if still_active.iter().any(|a| a == id) {
                write_volume_mute_lenient(id, *volume, *muted);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tone rendering + loopback capture (the automatic fan-out proof)
// ---------------------------------------------------------------------------

/// Play a 440 Hz sine on the CURRENT DEFAULT endpoint for `secs`, exactly
/// like an ordinary shared-mode app (16-bit 48 kHz stereo, OS-converted).
fn play_tone_on_default(secs: f32) {
    ensure_com();
    let result = (|| -> anyhow::Result<()> {
        let enumerator = enumerator();
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        let format = WAVEFORMATEX {
            wFormatTag: 1, // WAVE_FORMAT_PCM
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 192_000,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                2_000_000, // 200 ms
                0,
                &format,
                None,
            )
        }?;
        let render: IAudioRenderClient = unsafe { client.GetService() }?;
        let buffer_frames = unsafe { client.GetBufferSize() }?;
        unsafe { client.Start() }?;

        let total_frames = (secs * 48_000.0) as u64;
        let mut written: u64 = 0;
        let deadline = Instant::now() + Duration::from_secs_f32(secs + 3.0);
        while written < total_frames && Instant::now() < deadline {
            let padding = unsafe { client.GetCurrentPadding() }?;
            let free = buffer_frames.saturating_sub(padding) as u64;
            let want = free.min(total_frames - written) as u32;
            if want == 0 {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let data = unsafe { render.GetBuffer(want) }?;
            let samples =
                unsafe { std::slice::from_raw_parts_mut(data as *mut i16, want as usize * 2) };
            for frame in 0..want as usize {
                let t = (written + frame as u64) as f32 / 48_000.0;
                let value =
                    (0.8 * f32::sin(2.0 * std::f32::consts::PI * 440.0 * t) * 32_767.0) as i16;
                samples[frame * 2] = value;
                samples[frame * 2 + 1] = value;
            }
            unsafe { render.ReleaseBuffer(want, 0) }?;
            written += want as u64;
            std::thread::sleep(Duration::from_millis(10));
        }
        // Let the buffered tail drain before Stop so the device stays busy
        // for the whole advertised duration.
        std::thread::sleep(Duration::from_millis(200));
        let _ = unsafe { client.Stop() };
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("tone render failed: {e:#}");
    }
}

struct SignalStats {
    rms: f32,
    frames: u64,
}

/// Loopback-capture `id` for `secs` and measure the signal. Everything in the
/// endpoint's shared-mode mix lands here — for a secondary during fan-out,
/// that is exactly (and only) what the engine renders to it.
fn capture_signal(id: &str, secs: f32) -> anyhow::Result<SignalStats> {
    ensure_com();
    let enumerator = enumerator();
    let device = get_device(&enumerator, id);
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
    let raw = unsafe { client.GetMixFormat() }?;
    anyhow::ensure!(!raw.is_null(), "GetMixFormat returned null");
    let format = unsafe { raw.read_unaligned() };
    // Shared-mode mixes are 32-bit float in practice; the extensible tag's
    // subformat GUID (first 4 bytes = 3) or the plain IEEE_FLOAT tag both
    // mark it. Anything else would need format-specific decoding.
    let is_float = format.wFormatTag == 3
        || (format.wFormatTag == 0xFFFE && format.cbSize >= 22 && unsafe {
            // WAVEFORMATEXTENSIBLE.SubFormat: after wValidBitsPerSample (2)
            // and dwChannelMask (4), i.e. 6 bytes into the extension.
            let sub = (raw as *const u8).add(std::mem::size_of::<WAVEFORMATEX>() + 6)
                as *const GUID;
            sub.read_unaligned() == GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71)
        });
    let block_align = format.nBlockAlign as usize;
    let channels = format.nChannels as usize;
    unsafe { CoTaskMemFree(Some(raw as *const _)) };
    anyhow::ensure!(is_float, "unexpected non-float mix format on {id}");

    let raw = unsafe { client.GetMixFormat() }?; // fresh copy for Initialize
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            2_000_000,
            0,
            raw,
            None,
        )
    }?;
    unsafe { CoTaskMemFree(Some(raw as *const _)) };
    let capture: IAudioCaptureClient = unsafe { client.GetService() }?;
    unsafe { client.Start() }?;

    let mut sum_squares = 0f64;
    let mut samples: u64 = 0;
    let mut frames: u64 = 0;
    let deadline = Instant::now() + Duration::from_secs_f32(secs);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        loop {
            let packet = unsafe { capture.GetNextPacketSize() }?;
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut got: u32 = 0;
            let mut flags: u32 = 0;
            unsafe { capture.GetBuffer(&mut data, &mut got, &mut flags, None, None) }?;
            if !data.is_null() && got > 0 {
                let floats = unsafe {
                    std::slice::from_raw_parts(
                        data as *const f32,
                        got as usize * block_align / 4,
                    )
                };
                for &s in floats.iter().step_by(channels) {
                    sum_squares += (s as f64) * (s as f64);
                    samples += 1;
                }
                frames += got as u64;
            }
            unsafe { capture.ReleaseBuffer(got) }?;
        }
    }
    let _ = unsafe { client.Stop() };
    let rms = if samples == 0 {
        0.0
    } else {
        (sum_squares / samples as f64).sqrt() as f32
    };
    Ok(SignalStats { rms, frames })
}

// ---------------------------------------------------------------------------
// Event helpers
// ---------------------------------------------------------------------------

fn wait_for_event(
    rx: &mpsc::Receiver<BackendEvent>,
    what: &str,
    mut matches: impl FnMut(&BackendEvent) -> bool,
) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(event) if matches(&event) => return,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    panic!("no {what} event within {EVENT_TIMEOUT:?}");
}

/// Fail loudly on any backend Error event; they mean a stream died on real
/// hardware, which is exactly what this test exists to catch.
fn assert_no_errors(rx: &mpsc::Receiver<BackendEvent>, context: &str) {
    for event in rx.try_iter() {
        if let BackendEvent::Error(message) = event {
            panic!("backend error during {context}: {message}");
        }
    }
}

fn wait_until<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + APPLY_TIMEOUT;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(POLL);
    }
}

fn find<'a>(devices: &'a [Device], id: &str) -> &'a Device {
    devices
        .iter()
        .find(|d| d.id == id)
        .unwrap_or_else(|| panic!("device {id} missing from list"))
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[test]
#[ignore = "mutates live audio routing; run explicitly with --ignored"]
fn windows_live() {
    ensure_com();

    // ---- skip-gate -------------------------------------------------------
    let endpoints = active_endpoint_ids();
    if endpoints.is_empty() {
        eprintln!("SKIP: no active render endpoints");
        return;
    }
    let Some(original_default) = default_endpoint_id() else {
        eprintln!("SKIP: no default render endpoint");
        return;
    };

    let _guard = AudioStateGuard::capture();

    // ---- backend construction seeds the default as enabled ---------------
    let mut backend = WindowsBackend::new().expect("backend construction");
    let devices = backend.list_devices().expect("initial list_devices");
    assert!(!devices.is_empty(), "no devices listed");
    let mut ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), devices.len(), "duplicate device ids listed");
    assert!(
        devices.iter().all(|d| d.device_type != DeviceType::Master),
        "Windows must not emit a master row"
    );
    for d in &devices {
        assert!((0.0..=1.0).contains(&d.volume), "{}: volume {}", d.id, d.volume);
        assert!(!d.name.is_empty(), "{}: empty name", d.id);
    }
    assert!(
        find(&devices, &original_default).enabled,
        "the startup default endpoint must be seeded as enabled"
    );
    eprintln!("devices as listed by the backend:");
    for d in &devices {
        eprintln!(
            "  [{}] {:?} vol {:.2} muted {} enabled {} — {}",
            d.name, d.device_type, d.volume, d.muted, d.enabled, d.id
        );
    }

    // ---- monitoring ------------------------------------------------------
    let (tx, rx) = mpsc::channel();
    backend.start_monitoring(tx).expect("start_monitoring");

    // External volume change (null event context, like the system tray):
    // must come back as a VolumeChanged for that endpoint.
    let watch_id = original_default.clone();
    let (watch_volume, watch_muted) = read_volume_mute(&watch_id);
    let nudged = if watch_volume > 0.5 { watch_volume - 0.3 } else { watch_volume + 0.3 };
    write_volume_mute_lenient(&watch_id, nudged, watch_muted);
    wait_for_event(&rx, "external VolumeChanged", |e| {
        matches!(e, BackendEvent::VolumeChanged { id, .. } if *id == watch_id)
    });
    write_volume_mute_lenient(&watch_id, watch_volume, watch_muted);
    while rx.try_recv().is_ok() {} // drain the echo burst

    // ---- backend volume/mute writes land on the device -------------------
    let target = devices[0].id.clone();
    let (had_volume, had_muted) = read_volume_mute(&target);
    backend.set_volume(&target, 0.31).expect("set_volume");
    let (now_volume, _) = read_volume_mute(&target);
    assert!(
        (now_volume - 0.31).abs() < 0.05,
        "volume write did not land: wanted 0.31, device reports {now_volume}"
    );
    backend.set_muted(&target, true).expect("set_muted(true)");
    assert!(read_volume_mute(&target).1, "mute write did not land");
    backend.set_muted(&target, had_muted).expect("restore mute");
    backend.set_volume(&target, had_volume).expect("restore volume");

    // ---- single-device routing: plain default switch ---------------------
    // Applying a bogus id alongside must be ignored per the trait contract.
    backend
        .apply_enabled(&[original_default.clone(), "smx-bogus-endpoint-id".into()])
        .expect("apply_enabled(1 + bogus)");
    wait_until("default endpoint == primary", || {
        (default_endpoint_id().as_ref() == Some(&original_default)).then_some(())
    });
    let devices = backend.list_devices().expect("list after 1-device apply");
    assert!(find(&devices, &original_default).enabled);
    assert_eq!(
        devices.iter().filter(|d| d.enabled).count(),
        1,
        "exactly the primary must be enabled"
    );

    // ---- fan-out: 2+ devices, engine mirrors primary to secondary --------
    // Prefer a wired/display secondary over Bluetooth (deterministic timing)
    // and skip Hands-Free endpoints (enabling one flips the headset profile).
    let secondary = devices
        .iter()
        .filter(|d| d.id != original_default)
        .filter(|d| !d.name.to_ascii_lowercase().contains("hands-free"))
        .min_by_key(|d| (d.device_type == DeviceType::Bluetooth) as u8)
        .map(|d| d.id.clone());

    let Some(secondary) = secondary else {
        eprintln!("SKIP fan-out: only one usable render endpoint on this machine");
        drop(backend);
        return;
    };
    let primary = original_default.clone();
    eprintln!("fan-out: primary {primary} -> secondary {secondary}");

    // Known volumes for the signal check; the guard restores the originals.
    write_volume_mute_lenient(&primary, 0.25, false);
    write_volume_mute_lenient(&secondary, 0.50, false);
    while rx.try_recv().is_ok() {}

    backend
        .apply_enabled(&[primary.clone(), secondary.clone()])
        .expect("apply_enabled(2)");
    wait_until("default endpoint stays primary", || {
        (default_endpoint_id().as_ref() == Some(&primary)).then_some(())
    });
    let devices = backend.list_devices().expect("list during fan-out");
    assert!(find(&devices, &primary).enabled, "primary enabled");
    assert!(find(&devices, &secondary).enabled, "secondary enabled");
    assert_eq!(devices.iter().filter(|d| d.enabled).count(), 2);

    // The engine needs a moment to open its streams (Bluetooth especially).
    std::thread::sleep(Duration::from_millis(750));
    assert_no_errors(&rx, "engine spin-up");

    // The automatic listening test: tone on the primary (as any app would),
    // measured on the secondary via loopback. Only the engine connects them.
    let tone = std::thread::spawn(|| play_tone_on_default(TONE_SECS));
    std::thread::sleep(Duration::from_millis(1000)); // engine + BT ramp-up
    let stats = capture_signal(&secondary, CAPTURE_SECS).expect("loopback-capture secondary");
    tone.join().expect("tone thread");
    eprintln!(
        "secondary loopback: rms {:.4} over {} frames",
        stats.rms, stats.frames
    );
    assert!(
        stats.frames > 0,
        "no audio frames reached the secondary at all — engine renders nothing"
    );
    assert!(
        stats.rms > RMS_THRESHOLD,
        "tone did not reach the secondary (rms {:.5} <= {RMS_THRESHOLD}) — fan-out is silent",
        stats.rms
    );
    assert_no_errors(&rx, "fan-out playback");

    // ---- idempotent re-apply must not disturb the routing -----------------
    backend
        .apply_enabled(&[primary.clone(), secondary.clone()])
        .expect("idempotent re-apply");
    assert_eq!(default_endpoint_id().as_deref(), Some(primary.as_str()));
    let devices = backend.list_devices().expect("list after re-apply");
    assert_eq!(devices.iter().filter(|d| d.enabled).count(), 2);
    assert_no_errors(&rx, "idempotent re-apply");

    // ---- silent mode: 0 devices mutes the default, ownership honored ------
    backend.apply_enabled(&[]).expect("apply_enabled(0)");
    wait_until("default endpoint muted by silent mode", || {
        read_volume_mute(&primary).1.then_some(())
    });
    let devices = backend.list_devices().expect("list in silent mode");
    assert_eq!(
        devices.iter().filter(|d| d.enabled).count(),
        0,
        "silent mode must show zero enabled devices"
    );

    backend
        .apply_enabled(std::slice::from_ref(&primary))
        .expect("leave silent mode into 1 device");
    wait_until("silent-mode mute released", || {
        (!read_volume_mute(&primary).1).then_some(())
    });

    // ---- cleanup leaves a sane, unmuted default ---------------------------
    backend.cleanup().expect("cleanup");
    assert_eq!(
        default_endpoint_id().as_deref(),
        Some(primary.as_str()),
        "cleanup must leave a sane default endpoint"
    );
    assert!(
        !read_volume_mute(&primary).1,
        "cleanup must not leave our mute behind"
    );
    drop(backend);
    eprintln!("windows_live: all checks passed");
}
