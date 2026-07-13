use std::sync::{mpsc, Mutex};
use std::time::Duration;

use sound_multiplexer_audio::{
    compute_enabled_ids, create_backend, AudioBackend, BackendEvent, Device,
};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

/// The backend is `None` when the audio server was unreachable at startup;
/// `refresh_devices` retries creation so the user can fix the server and
/// hit Refresh instead of restarting the app.
struct AppState {
    backend: Mutex<Option<Box<dyn AudioBackend>>>,
    monitor_tx: mpsc::Sender<BackendEvent>,
}

type CmdResult<T> = Result<T, String>;

fn err_str(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn with_backend<T>(
    backend: &Mutex<Option<Box<dyn AudioBackend>>>,
    f: impl FnOnce(&mut Box<dyn AudioBackend>) -> anyhow::Result<T>,
) -> CmdResult<T> {
    let mut guard = backend.lock().unwrap();
    match guard.as_mut() {
        Some(backend) => f(backend).map_err(err_str),
        None => Err("audio backend unavailable".into()),
    }
}

/// Authoritative device state reaches the UI on exactly one path: the
/// `devices-changed` events emitted by [`event_pump`], which is a single
/// thread and therefore totally ordered. Mutating commands return no state;
/// they nudge the pump instead (see `notify`), so a slow command response
/// can never overwrite a fresher event in the UI. Only `get_devices` /
/// `refresh_devices` (initial load, explicit user refresh) return lists.
fn notify(monitor_tx: &mpsc::Sender<BackendEvent>) {
    let _ = monitor_tx.send(BackendEvent::DevicesChanged);
}

fn get_devices_inner(backend: &Mutex<Option<Box<dyn AudioBackend>>>) -> CmdResult<Vec<Device>> {
    with_backend(backend, |b| b.list_devices())
}

fn refresh_devices_inner(
    backend: &Mutex<Option<Box<dyn AudioBackend>>>,
    monitor_tx: &mpsc::Sender<BackendEvent>,
) -> CmdResult<Vec<Device>> {
    {
        let mut guard = backend.lock().unwrap();
        if guard.is_none() {
            match create_backend() {
                Ok(created) => *guard = Some(created),
                Err(e) => return Err(err_str(e)),
            }
        }
        // Restart-safe by contract: revives monitoring after the sound
        // server (and with it our subscription) went away and came back.
        if let Some(b) = guard.as_mut() {
            if let Err(e) = b.start_monitoring(monitor_tx.clone()) {
                log::warn!("monitoring unavailable: {e:#}");
            }
        }
    }
    with_backend(backend, |b| b.list_devices())
}

fn set_device_enabled_inner(
    backend: &Mutex<Option<Box<dyn AudioBackend>>>,
    monitor_tx: &mpsc::Sender<BackendEvent>,
    id: String,
    enabled: bool,
) -> CmdResult<()> {
    let result = with_backend(backend, |b| {
        let devices = b.list_devices()?;
        b.apply_enabled(&compute_enabled_ids(&devices, &id, enabled))
    });
    notify(monitor_tx);
    result
}

fn set_all_enabled_inner(
    backend: &Mutex<Option<Box<dyn AudioBackend>>>,
    monitor_tx: &mpsc::Sender<BackendEvent>,
    enabled: bool,
) -> CmdResult<()> {
    let result = with_backend(backend, |b| {
        let ids: Vec<String> = if enabled {
            b.list_devices()?.iter().map(|d| d.id.clone()).collect()
        } else {
            Vec::new()
        };
        b.apply_enabled(&ids)
    });
    notify(monitor_tx);
    result
}

fn set_device_volume_inner(
    backend: &Mutex<Option<Box<dyn AudioBackend>>>,
    id: String,
    volume: f32,
) -> CmdResult<()> {
    // No notify: during slider drags the monitor (Linux) or the optimistic
    // UI (Windows, which suppresses its own echo) keeps the UI truthful,
    // and pushing full snapshots at drag rate would only cause churn.
    with_backend(backend, |b| b.set_volume(&id, volume.clamp(0.0, 1.0)))
}

fn set_device_muted_inner(
    backend: &Mutex<Option<Box<dyn AudioBackend>>>,
    monitor_tx: &mpsc::Sender<BackendEvent>,
    id: String,
    muted: bool,
) -> CmdResult<()> {
    let result = with_backend(backend, |b| b.set_muted(&id, muted));
    notify(monitor_tx);
    result
}

/// Commands are `async` so they run on the Tauri async runtime instead of
/// the main/UI thread: every backend call may shell out (pactl) or block on
/// the backend mutex behind the event pump.
#[tauri::command]
async fn get_devices(state: State<'_, AppState>) -> CmdResult<Vec<Device>> {
    get_devices_inner(&state.backend)
}

#[tauri::command]
async fn refresh_devices(state: State<'_, AppState>) -> CmdResult<Vec<Device>> {
    refresh_devices_inner(&state.backend, &state.monitor_tx)
}

#[tauri::command]
async fn set_device_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> CmdResult<()> {
    set_device_enabled_inner(&state.backend, &state.monitor_tx, id, enabled)
}

#[tauri::command]
async fn set_all_enabled(state: State<'_, AppState>, enabled: bool) -> CmdResult<()> {
    set_all_enabled_inner(&state.backend, &state.monitor_tx, enabled)
}

#[tauri::command]
async fn set_device_volume(state: State<'_, AppState>, id: String, volume: f32) -> CmdResult<()> {
    set_device_volume_inner(&state.backend, id, volume)
}

#[tauri::command]
async fn set_device_muted(state: State<'_, AppState>, id: String, muted: bool) -> CmdResult<()> {
    set_device_muted_inner(&state.backend, &state.monitor_tx, id, muted)
}

/// Forwards backend notifications to the UI, coalescing event bursts
/// (a single sink change produces several pactl subscribe lines) into
/// one full-state push. Sole emitter of `devices-changed`.
fn event_pump(app: AppHandle, rx: mpsc::Receiver<BackendEvent>) {
    while let Ok(first) = rx.recv() {
        let mut error = match first {
            BackendEvent::Error(e) => Some(e),
            _ => None,
        };
        std::thread::sleep(Duration::from_millis(80));
        while let Ok(more) = rx.try_recv() {
            if let BackendEvent::Error(e) = more {
                error = Some(e);
            }
        }
        if let Some(e) = error {
            let _ = app.emit("backend-error", &e);
        }
        let state = app.state::<AppState>();
        let devices = {
            let mut guard = state.backend.lock().unwrap();
            guard.as_mut().map(|b| b.list_devices())
        };
        match devices {
            Some(Ok(devices)) => {
                let _ = app.emit("devices-changed", &devices);
            }
            Some(Err(e)) => {
                let _ = app.emit("backend-error", format!("{e:#}"));
            }
            None => {}
        }
    }
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let (monitor_tx, monitor_rx) = mpsc::channel();
    let monitor_tx_setup = monitor_tx.clone();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_devices,
            refresh_devices,
            set_device_enabled,
            set_all_enabled,
            set_device_volume,
            set_device_muted,
        ])
        .setup(move |app| {
            let backend = match create_backend() {
                Ok(mut backend) => {
                    if let Err(e) = backend.start_monitoring(monitor_tx_setup.clone()) {
                        log::warn!("monitoring unavailable: {e:#}");
                    }
                    Some(backend)
                }
                Err(e) => {
                    log::error!("audio backend unavailable: {e:#}");
                    None
                }
            };
            app.manage(AppState {
                backend: Mutex::new(backend),
                monitor_tx: monitor_tx_setup,
            });
            let handle = app.handle().clone();
            std::thread::spawn(move || event_pump(handle, monitor_rx));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                let state = app.state::<AppState>();
                let mut guard = state.backend.lock().unwrap();
                if let Some(backend) = guard.as_mut() {
                    if let Err(e) = backend.cleanup() {
                        log::error!("cleanup failed: {e:#}");
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sound_multiplexer_audio::DeviceType;
    use std::sync::Arc;

    /// In-memory backend that records every call. `fail` makes the mutating
    /// calls (`apply_enabled`, `set_muted`) error, to exercise the
    /// notify-even-on-failure paths.
    struct MockBackend {
        devices: Vec<Device>,
        calls: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl AudioBackend for MockBackend {
        fn list_devices(&mut self) -> anyhow::Result<Vec<Device>> {
            self.calls.lock().unwrap().push("list_devices".into());
            Ok(self.devices.clone())
        }

        fn apply_enabled(&mut self, ids: &[String]) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("apply_enabled({})", ids.join(",")));
            if self.fail {
                anyhow::bail!("apply failed");
            }
            for d in &mut self.devices {
                d.enabled = ids.contains(&d.id);
            }
            Ok(())
        }

        fn set_volume(&mut self, id: &str, volume: f32) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("set_volume({id},{volume:?})"));
            Ok(())
        }

        fn set_muted(&mut self, id: &str, muted: bool) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("set_muted({id},{muted})"));
            if self.fail {
                anyhow::bail!("mute failed");
            }
            Ok(())
        }

        fn start_monitoring(&mut self, _tx: mpsc::Sender<BackendEvent>) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push("start_monitoring".into());
            Ok(())
        }

        fn cleanup(&mut self) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push("cleanup".into());
            Ok(())
        }
    }

    /// The pieces of [`AppState`] the inner command fns operate on, plus
    /// the receiving end of the monitor channel and the mock's call log.
    struct Harness {
        backend: Mutex<Option<Box<dyn AudioBackend>>>,
        calls: Arc<Mutex<Vec<String>>>,
        tx: mpsc::Sender<BackendEvent>,
        rx: mpsc::Receiver<BackendEvent>,
    }

    fn harness(devices: Vec<Device>, fail: bool) -> Harness {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mock = MockBackend {
            devices,
            calls: Arc::clone(&calls),
            fail,
        };
        let (tx, rx) = mpsc::channel();
        Harness {
            backend: Mutex::new(Some(Box::new(mock))),
            calls,
            tx,
            rx,
        }
    }

    fn dev(id: &str, enabled: bool) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            device_type: DeviceType::Speakers,
            enabled,
            volume: 0.5,
            muted: false,
        }
    }

    fn devices_changed_count(rx: &mpsc::Receiver<BackendEvent>) -> usize {
        rx.try_iter()
            .filter(|e| matches!(e, BackendEvent::DevicesChanged))
            .count()
    }

    fn applied_sets(calls: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("apply_enabled("))
            .cloned()
            .collect()
    }

    #[test]
    fn set_device_enabled_toggle_on_appends_to_enabled_set() {
        let h = harness(vec![dev("a", true), dev("b", false)], false);
        set_device_enabled_inner(&h.backend, &h.tx, "b".into(), true).unwrap();
        assert_eq!(applied_sets(&h.calls), ["apply_enabled(a,b)"]);
        assert_eq!(devices_changed_count(&h.rx), 1);
    }

    #[test]
    fn set_device_enabled_toggle_off_removes_from_enabled_set() {
        let h = harness(vec![dev("a", true), dev("b", true)], false);
        set_device_enabled_inner(&h.backend, &h.tx, "a".into(), false).unwrap();
        assert_eq!(applied_sets(&h.calls), ["apply_enabled(b)"]);
        assert_eq!(devices_changed_count(&h.rx), 1);
    }

    #[test]
    fn set_device_enabled_toggle_on_already_enabled_does_not_duplicate() {
        let h = harness(vec![dev("a", true), dev("b", true)], false);
        set_device_enabled_inner(&h.backend, &h.tx, "b".into(), true).unwrap();
        assert_eq!(applied_sets(&h.calls), ["apply_enabled(a,b)"]);
        assert_eq!(devices_changed_count(&h.rx), 1);
    }

    #[test]
    fn set_all_enabled_true_applies_every_device_id() {
        let h = harness(
            vec![dev("a", true), dev("b", false), dev("c", false)],
            false,
        );
        set_all_enabled_inner(&h.backend, &h.tx, true).unwrap();
        assert_eq!(applied_sets(&h.calls), ["apply_enabled(a,b,c)"]);
        assert_eq!(devices_changed_count(&h.rx), 1);
    }

    #[test]
    fn set_all_enabled_false_applies_empty_set() {
        let h = harness(vec![dev("a", true), dev("b", true)], false);
        set_all_enabled_inner(&h.backend, &h.tx, false).unwrap();
        assert_eq!(applied_sets(&h.calls), ["apply_enabled()"]);
        assert_eq!(devices_changed_count(&h.rx), 1);
    }

    #[test]
    fn set_device_muted_notifies_exactly_once_on_success() {
        let h = harness(vec![dev("a", true)], false);
        set_device_muted_inner(&h.backend, &h.tx, "a".into(), true).unwrap();
        assert!(h
            .calls
            .lock()
            .unwrap()
            .contains(&"set_muted(a,true)".to_string()));
        assert_eq!(devices_changed_count(&h.rx), 1);
    }

    #[test]
    fn mutating_commands_notify_exactly_once_even_when_backend_fails() {
        let h = harness(vec![dev("a", true)], true);

        assert!(set_device_enabled_inner(&h.backend, &h.tx, "a".into(), false).is_err());
        assert_eq!(devices_changed_count(&h.rx), 1);

        assert!(set_all_enabled_inner(&h.backend, &h.tx, true).is_err());
        assert_eq!(devices_changed_count(&h.rx), 1);

        assert!(set_device_muted_inner(&h.backend, &h.tx, "a".into(), true).is_err());
        assert_eq!(devices_changed_count(&h.rx), 1);
    }

    #[test]
    fn set_device_volume_clamps_to_unit_range_and_sends_no_event() {
        let h = harness(vec![dev("a", true)], false);
        set_device_volume_inner(&h.backend, "a".into(), 1.5).unwrap();
        set_device_volume_inner(&h.backend, "a".into(), -0.25).unwrap();
        set_device_volume_inner(&h.backend, "a".into(), 0.5).unwrap();
        assert_eq!(
            *h.calls.lock().unwrap(),
            [
                "set_volume(a,1.0)",
                "set_volume(a,0.0)",
                "set_volume(a,0.5)"
            ]
        );
        assert_eq!(h.rx.try_iter().count(), 0);
    }

    #[test]
    fn commands_without_backend_report_unavailable() {
        let backend: Mutex<Option<Box<dyn AudioBackend>>> = Mutex::new(None);
        let (tx, _rx) = mpsc::channel();
        let err = "audio backend unavailable";
        assert_eq!(get_devices_inner(&backend).unwrap_err(), err);
        assert_eq!(
            set_device_enabled_inner(&backend, &tx, "a".into(), true).unwrap_err(),
            err
        );
        assert_eq!(set_all_enabled_inner(&backend, &tx, true).unwrap_err(), err);
        assert_eq!(
            set_device_volume_inner(&backend, "a".into(), 0.5).unwrap_err(),
            err
        );
        assert_eq!(
            set_device_muted_inner(&backend, &tx, "a".into(), true).unwrap_err(),
            err
        );
    }

    #[test]
    fn refresh_restarts_monitoring_on_existing_backend() {
        // `create_backend()` can't be faked, so only the existing-backend
        // branch is covered: refresh must revive monitoring and return the
        // current device list.
        let h = harness(vec![dev("a", true), dev("b", false)], false);
        let devices = refresh_devices_inner(&h.backend, &h.tx).unwrap();
        assert_eq!(devices, vec![dev("a", true), dev("b", false)]);
        assert!(h
            .calls
            .lock()
            .unwrap()
            .contains(&"start_monitoring".to_string()));
    }
}
