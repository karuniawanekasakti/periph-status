//! Background polling loop: scans devices, stores a snapshot, emits events to the UI,
//! updates the tray tooltip and sends low-battery notifications.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

use crate::alerts::{evaluate_low, tooltip_text};
use crate::model::Device;
use crate::provider::{self, BatteryProvider};
use crate::settings::Settings;

pub const EVENT_DEVICES_UPDATED: &str = "devices-updated";
pub const TRAY_ID: &str = "main";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// `false` until the first scan has finished.
    pub scanned: bool,
    pub devices: Vec<Device>,
    /// Unix epoch (seconds) of the last scan.
    pub updated_at: Option<u64>,
    pub error: Option<String>,
}

pub struct Monitor {
    snapshot: Mutex<Snapshot>,
    settings: Mutex<Settings>,
    /// Device id → already notified (currently below the threshold).
    alerted: Mutex<HashMap<String, bool>>,
    wake: Mutex<bool>,
    cv: Condvar,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Monitor {
    pub fn new(settings: Settings) -> Arc<Self> {
        Arc::new(Self {
            snapshot: Mutex::new(Snapshot::default()),
            settings: Mutex::new(settings),
            alerted: Mutex::new(HashMap::new()),
            wake: Mutex::new(false),
            cv: Condvar::new(),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.snapshot).clone()
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    pub fn set_settings(&self, s: Settings) {
        *lock(&self.settings) = s;
    }

    /// Request a scan as soon as possible (without waiting for the interval).
    pub fn refresh(&self) {
        *lock(&self.wake) = true;
        self.cv.notify_one();
    }

    pub fn start(self: &Arc<Self>, app: AppHandle) {
        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("battery-monitor".into())
            .spawn(move || {
                provider::init_thread();
                let mut provider = provider::default_provider();
                loop {
                    this.tick(&app, provider.as_mut());
                    this.wait();
                }
            })
            .expect("failed to spawn the monitor thread");
    }

    fn wait(&self) {
        let interval = Duration::from_secs(self.settings().interval_secs);
        let guard = lock(&self.wake);
        let (mut woken, _) = self
            .cv
            .wait_timeout_while(guard, interval, |woken| !*woken)
            .unwrap_or_else(|e| e.into_inner());
        *woken = false;
    }

    fn tick(&self, app: &AppHandle, provider: &mut dyn BatteryProvider) {
        let result = provider.scan();
        let settings = self.settings();

        let snapshot = {
            let mut snap = lock(&self.snapshot);
            match result {
                Ok(devices) => {
                    self.check_low_battery(app, &devices, &settings);
                    snap.devices = devices;
                    snap.error = None;
                }
                Err(e) => {
                    log::error!("scan failed: {e:#}");
                    snap.error = Some(format!("{e:#}"));
                }
            }
            snap.scanned = true;
            snap.updated_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs());
            snap.clone()
        };

        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let _ = tray.set_tooltip(Some(tooltip_text(&snapshot.devices)));
        }
        let _ = app.emit(EVENT_DEVICES_UPDATED, &snapshot);
    }

    fn check_low_battery(&self, app: &AppHandle, devices: &[Device], settings: &Settings) {
        let mut alerted = lock(&self.alerted);
        for d in devices.iter().filter(|d| d.connected) {
            let was = alerted.get(&d.id).copied().unwrap_or(false);
            let (notify, now) = evaluate_low(was, d.battery, settings.threshold);
            alerted.insert(d.id.clone(), now);
            if notify && settings.notify {
                let level = d.battery.unwrap_or_default();
                let res = app
                    .notification()
                    .builder()
                    .title("Low battery")
                    .body(format!("{} is at {}%. Charge it soon.", d.name, level))
                    .show();
                if let Err(e) = res {
                    log::warn!("failed to send notification: {e}");
                }
            }
        }
    }
}
