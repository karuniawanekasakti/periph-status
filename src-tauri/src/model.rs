use serde::{Deserialize, Serialize};

/// A peripheral the OS knows about (paired Bluetooth device or 2.4 GHz receiver slot).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable id from the OS (on Windows: the AssociationEndpoint id).
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// Battery percentage 0..=100, `None` if the device does not report it.
    pub battery: Option<u8>,
    pub connected: bool,
    pub source: BatterySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
    Gamepad,
    Phone,
    Computer,
    Other,
}

/// Where the battery value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatterySource {
    /// PnP property, the same one Windows Settings uses.
    Pnp,
    /// Read directly from the GATT Battery Service (BLE).
    Gatt,
    /// Read from a 2.4 GHz receiver/dongle through a vendor protocol.
    Dongle,
    None,
}

/// Device kind from the Class of Device (Bluetooth classic, raw 24-bit value).
pub fn kind_from_cod(raw: u32) -> DeviceKind {
    let major = (raw >> 8) & 0x1F;
    let minor = (raw >> 2) & 0x3F;
    match major {
        1 => DeviceKind::Computer,
        2 => DeviceKind::Phone,
        4 => DeviceKind::Headset,
        5 => {
            let keyboard = minor & 0x10 != 0;
            let pointing = minor & 0x20 != 0;
            match (keyboard, pointing, minor & 0x0F) {
                (true, _, _) => DeviceKind::Keyboard,
                (false, true, _) => DeviceKind::Mouse,
                (false, false, 1 | 2) => DeviceKind::Gamepad,
                _ => DeviceKind::Other,
            }
        }
        _ => DeviceKind::Other,
    }
}

/// Device kind from the BLE Appearance (category / subcategory).
pub fn kind_from_appearance(category: u16, subcategory: u16) -> DeviceKind {
    match (category, subcategory) {
        (0x001, _) => DeviceKind::Phone,
        (0x002, _) => DeviceKind::Computer,
        (0x00F, 1) => DeviceKind::Keyboard,
        (0x00F, 2) => DeviceKind::Mouse,
        (0x00F, 3 | 4) => DeviceKind::Gamepad,
        // Audio categories in the Bluetooth Assigned Numbers (audio sink/source, earbud, headset, ...)
        (0x021..=0x025, _) => DeviceKind::Headset,
        _ => DeviceKind::Other,
    }
}

/// Last-resort guess from the device name when the OS gives no kind information.
pub fn kind_from_name(name: &str) -> DeviceKind {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["mouse", "trackball", "trackpad"]) {
        DeviceKind::Mouse
    } else if has(&["keyboard", "keys", "k380", "k780"]) {
        DeviceKind::Keyboard
    } else if has(&[
        "buds", "airpods", "headphone", "headset", "earbud", "earphone", "tws", "wh-", "wf-", "jbl",
    ]) {
        DeviceKind::Headset
    } else if has(&["controller", "gamepad", "joy-con", "dualsense", "dualshock"]) {
        DeviceKind::Gamepad
    } else {
        DeviceKind::Other
    }
}

/// Sort connected devices first, then by name.
pub fn sort_devices(devices: &mut [Device]) {
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cod_peripherals() {
        // 0x002580 = Peripheral, pointing device (mouse)
        assert_eq!(kind_from_cod(0x002580), DeviceKind::Mouse);
        // 0x002540 = Peripheral, keyboard
        assert_eq!(kind_from_cod(0x002540), DeviceKind::Keyboard);
        // 0x0025C0 = Peripheral, combo keyboard/pointing
        assert_eq!(kind_from_cod(0x0025C0), DeviceKind::Keyboard);
        // 0x002508 = Peripheral, gamepad
        assert_eq!(kind_from_cod(0x002508), DeviceKind::Gamepad);
        // 0x240404 = Audio/Video, wearable headset
        assert_eq!(kind_from_cod(0x240404), DeviceKind::Headset);
        // 0x5A020C = Phone, smartphone
        assert_eq!(kind_from_cod(0x5A020C), DeviceKind::Phone);
    }

    #[test]
    fn appearance() {
        assert_eq!(kind_from_appearance(0x00F, 2), DeviceKind::Mouse);
        assert_eq!(kind_from_appearance(0x00F, 1), DeviceKind::Keyboard);
        assert_eq!(kind_from_appearance(0x025, 1), DeviceKind::Headset);
        assert_eq!(kind_from_appearance(0x000, 0), DeviceKind::Other);
    }

    #[test]
    fn names() {
        assert_eq!(kind_from_name("MX Master 3S"), DeviceKind::Other);
        assert_eq!(kind_from_name("Logitech Pebble Mouse"), DeviceKind::Mouse);
        assert_eq!(kind_from_name("Galaxy Buds2 Pro"), DeviceKind::Headset);
        assert_eq!(kind_from_name("Keychron K2"), DeviceKind::Other);
        assert_eq!(kind_from_name("Xbox Wireless Controller"), DeviceKind::Gamepad);
    }

    #[test]
    fn sorting() {
        let d = |name: &str, connected| Device {
            id: name.into(),
            name: name.into(),
            kind: DeviceKind::Other,
            battery: None,
            connected,
            source: BatterySource::None,
        };
        let mut v = vec![d("b", false), d("C", true), d("a", true)];
        sort_devices(&mut v);
        let names: Vec<_> = v.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["a", "C", "b"]);
    }
}
