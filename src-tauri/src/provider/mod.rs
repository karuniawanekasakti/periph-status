//! Per-platform battery data sources. Each OS implements [`BatteryProvider`].

use std::collections::HashMap;

use crate::model::{kind_from_name, sort_devices, BatterySource, Device, DeviceKind};

#[cfg(target_os = "macos")]
mod mac;
pub mod huawei;
pub(crate) mod rongyuan;
#[cfg(windows)]
mod win;

pub trait BatteryProvider {
    /// Return the known devices with their battery levels.
    /// May block; always called from a background thread.
    fn scan(&mut self) -> anyhow::Result<Vec<Device>>;
}

/// Call once at the start of the thread that will run `scan()`.
pub fn init_thread() {
    #[cfg(windows)]
    win::init_thread();
}

pub fn default_provider() -> Box<dyn BatteryProvider> {
    #[cfg(windows)]
    return Box::new(win::WindowsProvider::new());
    #[cfg(target_os = "macos")]
    return Box::new(mac::MacProvider);
    #[cfg(not(any(windows, target_os = "macos")))]
    return Box::new(NullProvider);
}

#[allow(dead_code)]
pub struct NullProvider;

impl BatteryProvider for NullProvider {
    fn scan(&mut self) -> anyhow::Result<Vec<Device>> {
        Ok(Vec::new())
    }
}

/// Raw enumeration result before it is merged with battery data.
#[derive(Debug, Clone)]
pub(crate) struct RawEntry {
    pub id: String,
    pub name: String,
    /// ContainerId: one physical device can have several endpoints (classic + LE)
    /// and several devnodes; they all share the same ContainerId.
    pub container: Option<u128>,
    pub connected: bool,
    pub kind: DeviceKind,
    /// Battery value from a non-PnP source (e.g. GATT), if it was read.
    pub gatt_battery: Option<u8>,
}

/// Combine endpoints with the PnP battery map (ContainerId -> %), and merge
/// endpoints that belong to the same physical device.
pub(crate) fn merge(entries: Vec<RawEntry>, pnp_battery: &HashMap<u128, u8>) -> Vec<Device> {
    let mut out: Vec<Device> = Vec::new();
    let mut by_container: HashMap<u128, usize> = HashMap::new();

    for e in entries {
        let pnp = e.container.and_then(|c| pnp_battery.get(&c).copied());
        let (battery, source) = match (pnp, e.gatt_battery) {
            (Some(b), _) => (Some(b), BatterySource::Pnp),
            (None, Some(b)) => (Some(b), BatterySource::Gatt),
            (None, None) => (None, BatterySource::None),
        };
        let kind = match e.kind {
            DeviceKind::Other => kind_from_name(&e.name),
            k => k,
        };

        if let Some(&idx) = e.container.as_ref().and_then(|c| by_container.get(c)) {
            let d = &mut out[idx];
            d.connected |= e.connected;
            if d.battery.is_none() && battery.is_some() {
                d.battery = battery;
                d.source = source;
            }
            if d.kind == DeviceKind::Other {
                d.kind = kind;
            }
            continue;
        }

        if let Some(c) = e.container {
            by_container.insert(c, out.len());
        }
        out.push(Device {
            id: e.id,
            name: e.name,
            kind,
            battery: battery.map(|b| b.min(100)),
            connected: e.connected,
            source,
            details: None,
        });
    }

    sort_devices(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, container: Option<u128>, connected: bool) -> RawEntry {
        RawEntry {
            id: id.into(),
            name: id.into(),
            container,
            connected,
            kind: DeviceKind::Other,
            gatt_battery: None,
        }
    }

    #[test]
    fn maps_pnp_battery_by_container() {
        let pnp = HashMap::from([(1u128, 80u8)]);
        let out = merge(vec![entry("mouse", Some(1), true), entry("kbd", Some(2), true)], &pnp);
        let mouse = out.iter().find(|d| d.id == "mouse").unwrap();
        assert_eq!(mouse.battery, Some(80));
        assert_eq!(mouse.source, BatterySource::Pnp);
        let kbd = out.iter().find(|d| d.id == "kbd").unwrap();
        assert_eq!(kbd.battery, None);
        assert_eq!(kbd.source, BatterySource::None);
    }

    #[test]
    fn pnp_wins_over_gatt_and_gatt_is_fallback() {
        let pnp = HashMap::from([(1u128, 55u8)]);
        let mut a = entry("a", Some(1), true);
        a.gatt_battery = Some(10);
        let mut b = entry("b", Some(2), true);
        b.gatt_battery = Some(42);
        let out = merge(vec![a, b], &pnp);
        assert_eq!(out.iter().find(|d| d.id == "a").unwrap().battery, Some(55));
        let b = out.iter().find(|d| d.id == "b").unwrap();
        assert_eq!((b.battery, b.source), (Some(42), BatterySource::Gatt));
    }

    #[test]
    fn dual_mode_device_is_merged() {
        // Classic endpoint (disconnected) + LE endpoint (connected) for the same device.
        let pnp = HashMap::from([(7u128, 64u8)]);
        let mut le = entry("buds-le", Some(7), true);
        le.kind = DeviceKind::Headset;
        let out = merge(vec![entry("buds", Some(7), false), le], &pnp);
        assert_eq!(out.len(), 1);
        assert!(out[0].connected);
        assert_eq!(out[0].battery, Some(64));
        assert_eq!(out[0].kind, DeviceKind::Headset);
    }

    #[test]
    fn entries_without_container_are_kept_separately() {
        let out = merge(vec![entry("x", None, false), entry("y", None, false)], &HashMap::new());
        assert_eq!(out.len(), 2);
    }
}
