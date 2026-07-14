//! Loopback fan-out engine: captures the shared-mode mix of the primary
//! render endpoint (`AUDCLNT_STREAMFLAGS_LOOPBACK`) and re-renders it to
//! each secondary endpoint, with per-device ring buffers, silence
//! injection when the source goes quiet, and buffer-occupancy based
//! drift handling.
//!
//! Windows realities encoded here:
//! - Loopback capture only sees the shared-mode mix. Exclusive-mode and
//!   DRM-protected streams bypass the mix and are mirrored as silence.
//! - Secondary outputs lag the primary by roughly the ring target (~60 ms).
//!   That is accepted v1 behavior; `IAudioClockAdjustment`-based rate
//!   matching is the planned refinement over the occupancy-clamp drift
//!   strategy implemented in [`Ring::push`].
//! - Event-driven loopback capture is historically unreliable when the
//!   endpoint has no active render stream (the event may never fire), so
//!   the capture wait uses a short timeout and drains on the timer too.
//!
//! All buffers carry raw bytes of the *capture* mix format; render streams
//! open with that same format plus `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
//! SRC_DEFAULT_QUALITY`, so the OS converts to each device's own format and
//! the engine never touches samples. Frame alignment is `nBlockAlign`;
//! frames are never split.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Context as _;
use log::{debug, info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, IAudioRenderClient, WAVEFORMATEX,
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::{create_enumerator, ensure_com, get_device};
use crate::fanout::{FrameLayout, Ring};
use crate::BackendEvent;

/// Steady-state ring fill: the secondary-output lag.
const RING_TARGET_MS: usize = 60;
/// Occupancy ceiling; above this the oldest frames are dropped back to target.
const RING_MAX_MS: usize = 120;
/// WASAPI buffer duration for both streams, in 100 ns units (100 ms).
const BUFFER_DURATION_100NS: i64 = 1_000_000;
/// Capture wait: doubles as the poll interval for the flaky-loopback-event
/// workaround (see module docs).
const CAPTURE_WAIT_MS: u32 = 10;
/// Render wait: events fire every device period (~10 ms); the timeout only
/// bounds stop latency.
const RENDER_WAIT_MS: u32 = 100;

// ---------------------------------------------------------------------------
// Mix format
// ---------------------------------------------------------------------------

/// Owned copy of the primary endpoint's mix format (`WAVEFORMATEX` header
/// plus its `cbSize` extension), shareable across threads as plain bytes.
/// The byte/frame math lives in the platform-neutral [`FrameLayout`].
#[derive(Clone)]
struct MixFormat {
    bytes: Vec<u8>,
    layout: FrameLayout,
}

impl MixFormat {
    /// Copy out of (and free) a `GetMixFormat` CoTaskMem allocation.
    ///
    /// # Safety
    /// `raw` must be a valid `GetMixFormat` result or null.
    unsafe fn from_raw(raw: *mut WAVEFORMATEX) -> anyhow::Result<Self> {
        anyhow::ensure!(!raw.is_null(), "GetMixFormat returned null");
        // WAVEFORMATEX is packed(1): copy the header out before reading fields.
        let header = unsafe { raw.read_unaligned() };
        let len = std::mem::size_of::<WAVEFORMATEX>() + header.cbSize as usize;
        let bytes = unsafe { std::slice::from_raw_parts(raw as *const u8, len) }.to_vec();
        unsafe { CoTaskMemFree(Some(raw as *const _)) };
        let block_align = header.nBlockAlign as usize;
        let avg_bytes_per_sec = header.nAvgBytesPerSec as usize;
        anyhow::ensure!(block_align > 0, "mix format has zero block align");
        anyhow::ensure!(avg_bytes_per_sec > 0, "mix format has zero byte rate");
        Ok(Self {
            bytes,
            layout: FrameLayout {
                block_align,
                avg_bytes_per_sec,
            },
        })
    }

    fn as_wfx(&self) -> *const WAVEFORMATEX {
        // WAVEFORMATEX is packed(1), so byte alignment of Vec<u8> suffices.
        self.bytes.as_ptr() as *const WAVEFORMATEX
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Shared per-stream context handed to capture/render threads. Threads create
/// all their COM objects locally (from the endpoint id), so nothing COM ever
/// crosses a thread boundary here.
struct StreamCtx {
    id: String,
    format: MixFormat,
    /// Engine-wide shutdown flag; every stream loop exits once it is set.
    stop: Arc<AtomicBool>,
    /// Raised (never cleared) when any stream dies; `Engine::is_running`
    /// then turns false so the backend rebuilds routing (see [`fail_stream`]).
    failed: Arc<AtomicBool>,
    /// Whether this stream's failure stops the whole engine: true for the
    /// capture stream (the single source every secondary depends on), false
    /// for render streams (a dead secondary must not silence its siblings).
    fatal: bool,
    tx: Option<Sender<BackendEvent>>,
}

pub struct Engine {
    primary: String,
    secondaries: Vec<String>,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Engine {
    /// Spawn the loopback capture thread for `primary` and one render thread
    /// per secondary. `tx` carries failure events (device invalidation etc.).
    pub fn start(
        primary: String,
        secondaries: Vec<String>,
        tx: Option<Sender<BackendEvent>>,
    ) -> anyhow::Result<Self> {
        ensure_com();
        // Read the capture mix format up front: every render stream opens
        // with this exact format so frames pass through unconverted.
        let format = {
            let enumerator = create_enumerator()?;
            let device = get_device(&enumerator, &primary)?;
            let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
                .context("activate IAudioClient on primary")?;
            let raw = unsafe { client.GetMixFormat() }.context("GetMixFormat on primary")?;
            unsafe { MixFormat::from_raw(raw) }?
        };

        let stop = Arc::new(AtomicBool::new(false));
        // Build the Engine before spawning anything: if a later spawn fails,
        // dropping this partially populated Engine on the error return stops
        // and joins the threads spawned so far (closing their WASAPI streams)
        // instead of leaking them for the rest of the process lifetime.
        let mut engine = Self {
            primary: primary.clone(),
            secondaries: secondaries.clone(),
            stop: Arc::clone(&stop),
            failed: Arc::new(AtomicBool::new(false)),
            threads: Vec::with_capacity(secondaries.len() + 1),
        };
        let mut rings = Vec::with_capacity(secondaries.len());

        for (n, secondary) in secondaries.into_iter().enumerate() {
            let ring = Arc::new(Ring::new(&format.layout, RING_TARGET_MS, RING_MAX_MS));
            rings.push(Arc::clone(&ring));
            let ctx = StreamCtx {
                id: secondary,
                format: format.clone(),
                stop: Arc::clone(&stop),
                failed: Arc::clone(&engine.failed),
                fatal: false,
                tx: tx.clone(),
            };
            let thread = std::thread::Builder::new()
                .name(format!("smx-render-{n}"))
                .spawn(move || render_thread(ctx, ring))
                .context("spawn render thread")?;
            engine.threads.push(thread);
        }

        let ctx = StreamCtx {
            id: primary,
            format,
            stop,
            failed: Arc::clone(&engine.failed),
            fatal: true,
            tx,
        };
        let thread = std::thread::Builder::new()
            .name("smx-capture".into())
            .spawn(move || capture_thread(ctx, rings))
            .context("spawn capture thread")?;
        engine.threads.push(thread);

        info!(
            "loopback engine started: primary {}, {} secondaries",
            engine.primary,
            engine.secondaries.len()
        );
        Ok(engine)
    }

    pub fn primary(&self) -> &str {
        &self.primary
    }

    pub fn secondaries(&self) -> &[String] {
        &self.secondaries
    }

    /// False once any stream died (device invalidated etc.). The backend's
    /// reconcile path reacts by rebuilding the engine against the surviving
    /// devices (see `WindowsBackend::list_devices`).
    pub fn is_running(&self) -> bool {
        !self.stop.load(Ordering::SeqCst) && !self.failed.load(Ordering::SeqCst)
    }

    fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            if thread.join().is_err() {
                warn!("engine thread panicked during shutdown");
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_and_join();
        debug!("loopback engine stopped");
    }
}

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

/// Auto-reset event for `AUDCLNT_STREAMFLAGS_EVENTCALLBACK`, closed on drop.
struct EventHandle(HANDLE);

impl EventHandle {
    fn new() -> anyhow::Result<Self> {
        let handle =
            unsafe { CreateEventW(None, false, false, None) }.context("CreateEventW")?;
        Ok(Self(handle))
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        if let Err(e) = unsafe { CloseHandle(self.0) } {
            debug!("CloseHandle on stream event: {e}");
        }
    }
}

/// Report a stream failure. A dying render stream takes down only itself:
/// capture and the other secondaries keep playing, degraded. A capture
/// failure stops the whole engine — without the source every secondary is
/// pointless. Either way `failed` is raised and a DevicesChanged event is
/// emitted; the backend re-lists devices on it and rebuilds the engine
/// against the surviving devices (see `WindowsBackend::list_devices`).
fn fail_stream(ctx: &StreamCtx, what: &str, error: anyhow::Error) {
    let shutting_down = if ctx.fatal {
        ctx.stop.swap(true, Ordering::SeqCst)
    } else {
        ctx.stop.load(Ordering::SeqCst)
    };
    if shutting_down {
        // Already shutting down; errors here are teardown fallout.
        debug!("{what} on {} during shutdown: {error:#}", ctx.id);
        return;
    }
    ctx.failed.store(true, Ordering::SeqCst);
    let invalidated = error
        .downcast_ref::<windows::core::Error>()
        .is_some_and(|e| e.code() == AUDCLNT_E_DEVICE_INVALIDATED);
    warn!("{what} on {} stopped: {error:#}", ctx.id);
    if let Some(tx) = &ctx.tx {
        let message = if invalidated {
            format!("Audio device disappeared during playback ({})", ctx.id)
        } else {
            format!("{what} failed on {}: {error}", ctx.id)
        };
        let _ = tx.send(BackendEvent::Error(message));
        let _ = tx.send(BackendEvent::DevicesChanged);
    }
}

fn capture_thread(ctx: StreamCtx, rings: Vec<Arc<Ring>>) {
    ensure_com();
    if let Err(e) = run_capture(&ctx, &rings) {
        fail_stream(&ctx, "loopback capture", e);
    }
}

fn run_capture(ctx: &StreamCtx, rings: &[Arc<Ring>]) -> anyhow::Result<()> {
    let enumerator = create_enumerator()?;
    let device = get_device(&enumerator, &ctx.id)?;
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.context("activate IAudioClient")?;
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            BUFFER_DURATION_100NS,
            0,
            ctx.format.as_wfx(),
            None,
        )
    }
    .context("initialize loopback capture stream")?;
    let event = EventHandle::new()?;
    unsafe { client.SetEventHandle(event.0) }.context("SetEventHandle")?;
    let capture: IAudioCaptureClient =
        unsafe { client.GetService() }.context("get IAudioCaptureClient")?;
    unsafe { client.Start() }.context("start capture stream")?;

    let frame_size = ctx.format.layout.block_align;
    let mut silence: Vec<u8> = Vec::new();
    while !ctx.stop.load(Ordering::SeqCst) {
        let wait = unsafe { WaitForSingleObject(event.0, CAPTURE_WAIT_MS) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT {
            anyhow::bail!("wait on capture event failed: {wait:?}");
        }
        // Drain every pending packet, event-signaled or not.
        loop {
            let packet_frames =
                unsafe { capture.GetNextPacketSize() }.context("GetNextPacketSize")?;
            if packet_frames == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
                .context("capture GetBuffer")?;
            let len = frames as usize * frame_size;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                // Silence is data too: keep the rings fed so secondaries stay
                // in step instead of underrunning at a random offset.
                if silence.len() < len {
                    silence.resize(len, 0);
                }
                for ring in rings {
                    ring.push(&silence[..len]);
                }
            } else if !data.is_null() && len > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(data, len) };
                for ring in rings {
                    ring.push(bytes);
                }
            }
            if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                debug!("capture discontinuity on {}", ctx.id);
            }
            unsafe { capture.ReleaseBuffer(frames) }.context("capture ReleaseBuffer")?;
        }
    }
    let _ = unsafe { client.Stop() };
    Ok(())
}

fn render_thread(ctx: StreamCtx, ring: Arc<Ring>) {
    ensure_com();
    if let Err(e) = run_render(&ctx, &ring) {
        fail_stream(&ctx, "render", e);
    }
}

fn run_render(ctx: &StreamCtx, ring: &Ring) -> anyhow::Result<()> {
    let enumerator = create_enumerator()?;
    let device = get_device(&enumerator, &ctx.id)?;
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.context("activate IAudioClient")?;
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            BUFFER_DURATION_100NS,
            0,
            // The *capture* mix format: the OS converts to the device format.
            ctx.format.as_wfx(),
            None,
        )
    }
    .context("initialize render stream")?;
    let event = EventHandle::new()?;
    unsafe { client.SetEventHandle(event.0) }.context("SetEventHandle")?;
    let render: IAudioRenderClient =
        unsafe { client.GetService() }.context("get IAudioRenderClient")?;
    let buffer_frames = unsafe { client.GetBufferSize() }.context("GetBufferSize")?;
    unsafe { client.Start() }.context("start render stream")?;

    let frame_size = ctx.format.layout.block_align;
    while !ctx.stop.load(Ordering::SeqCst) {
        let wait = unsafe { WaitForSingleObject(event.0, RENDER_WAIT_MS) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT {
            anyhow::bail!("wait on render event failed: {wait:?}");
        }
        let padding = unsafe { client.GetCurrentPadding() }.context("GetCurrentPadding")?;
        let want_frames = buffer_frames.saturating_sub(padding);
        if want_frames == 0 {
            continue;
        }
        let data = unsafe { render.GetBuffer(want_frames) }.context("render GetBuffer")?;
        anyhow::ensure!(!data.is_null(), "render GetBuffer returned null");
        let out =
            unsafe { std::slice::from_raw_parts_mut(data, want_frames as usize * frame_size) };
        let filled = ring.pop_into(out);
        // Ring underrun (source quiet, capture gap, or startup): pad with
        // silence — zero samples are silence in every shared-mode PCM format.
        out[filled..].fill(0);
        unsafe { render.ReleaseBuffer(want_frames, 0) }.context("render ReleaseBuffer")?;
    }
    let _ = unsafe { client.Stop() };
    Ok(())
}
