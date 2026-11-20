//! 2.4 GHz receivers built on the Rongyuan chip (VID 0x3151), used by many wireless mice and
//! keyboards (Attack Shark, Ajazz, MonsGeek, INNO, ...). The battery is not exposed through
//! standard HID usages, but the receiver answers a status query on vendor collection FFFF:0002:
//!
//!   SET_FEATURE  [00 F7 00 00 …]        (length = feature report length)
//!   GET_FEATURE  report 0 → [00][ok][kbd %][mouse %][kbd offline][mouse offline]…
//!
//! Protocol and PID list follow qu1ncky/BatteryTray (MIT, derived from the official ATTACK SHARK
//! web driver) via krisgfx9/Wireless-Device-Battery-Monitor (`src/drivers/rongyuan.cpp`).

use crate::model::DeviceKind;

pub const VID: u16 = 0x3151;

/// Receiver PIDs known to speak this protocol.
pub const PIDS: &[u16] = &[
    0x4011, 0x4014, 0x4017, 0x4021, 0x4027, 0x402D, 0x4038, 0x5006, 0x5007, 0x5008, 0x5020, 0x5026,
    0x502B, 0x5031, 0x5037, 0x5038, 0x503D, 0x503F, 0x5040, 0x5042, 0x5045, 0x504B, 0x504E, 0x5050,
    0x5052, 0x5055, 0x505A,
];

pub const QUERY_CMD: u8 = 0xF7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub kind: DeviceKind,
    pub level: u8,
    /// `false` when the receiver reports the device offline (off/asleep); `level` is then the last known value.
    pub online: bool,
}

/// Extract battery levels from a status reply, for every slot with a plausible value (1..=100).
pub fn parse_reply(reply: &[u8]) -> Vec<Slot> {
    if reply.len() < 7 || reply[2..=6].iter().all(|&b| b == 0) {
        return Vec::new();
    }
    [
        (DeviceKind::Keyboard, reply[2], reply[4]),
        (DeviceKind::Mouse, reply[3], reply[5]),
    ]
    .into_iter()
    .filter(|&(_, level, _)| (1..=100).contains(&level))
    .map(|(kind, level, offline)| Slot { kind, level, online: offline == 0 })
    .collect()
}

#[cfg(windows)]
pub use win::read_all;

#[cfg(windows)]
mod win {
    use std::collections::HashSet;
    use std::time::Duration;

    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
    };
    use windows::Win32::Devices::HumanInterfaceDevice::{
        HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
        HidD_GetPreparsedData, HidD_GetProductString, HidD_SetFeature, HidP_GetCaps, HIDD_ATTRIBUTES,
        HIDP_CAPS, PHIDP_PREPARSED_DATA,
    };
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    use super::{parse_reply, PIDS, QUERY_CMD, VID};
    use crate::model::{BatterySource, Device};

    /// Handle that is closed on drop.
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn open(path: &str, access: u32) -> Option<Handle> {
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
            .ok()
            .map(Handle)
        }
    }

    fn hid_paths() -> Vec<String> {
        unsafe {
            let guid: GUID = HidD_GetHidGuid();
            let mut len = 0u32;
            if CM_Get_Device_Interface_List_SizeW(&mut len, &guid, PCWSTR::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT)
                != CR_SUCCESS
            {
                return Vec::new();
            }
            let mut buf = vec![0u16; len as usize];
            if CM_Get_Device_Interface_ListW(&guid, PCWSTR::null(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT)
                != CR_SUCCESS
            {
                return Vec::new();
            }
            buf.split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(String::from_utf16_lossy)
                .collect()
        }
    }

    /// Collection info `(pid, product name, feature report length)` if this is a receiver query collection.
    fn query_collection(h: &Handle) -> Option<(u16, String, usize)> {
        unsafe {
            let mut attrs = HIDD_ATTRIBUTES {
                Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                ..Default::default()
            };
            if !HidD_GetAttributes(h.0, &mut attrs) || attrs.VendorID != VID || !PIDS.contains(&attrs.ProductID) {
                return None;
            }
            let mut pp = PHIDP_PREPARSED_DATA::default();
            if !HidD_GetPreparsedData(h.0, &mut pp) {
                return None;
            }
            let mut caps = HIDP_CAPS::default();
            let _ = HidP_GetCaps(pp, &mut caps);
            let _ = HidD_FreePreparsedData(pp);
            if caps.UsagePage != 0xFFFF || caps.Usage != 0x0002 || caps.FeatureReportByteLength < 7 {
                return None;
            }
            let mut name = [0u16; 128];
            let product = if HidD_GetProductString(h.0, name.as_mut_ptr().cast(), 256) {
                let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                String::from_utf16_lossy(&name[..end]).trim().to_owned()
            } else {
                String::new()
            };
            Some((attrs.ProductID, product, caps.FeatureReportByteLength as usize))
        }
    }

    fn query(h: &Handle, len: usize) -> Option<Vec<u8>> {
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(100));
            }
            let mut cmd = vec![0u8; len];
            cmd[1] = QUERY_CMD;
            unsafe {
                if !HidD_SetFeature(h.0, cmd.as_ptr().cast(), len as u32) {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(20));
                let mut reply = vec![0u8; len];
                if HidD_GetFeature(h.0, reply.as_mut_ptr().cast(), len as u32) && !parse_reply(&reply).is_empty() {
                    return Some(reply);
                }
            }
        }
        None
    }

    /// Read every plugged-in Rongyuan receiver. Devices that are currently offline are returned
    /// with `connected = false` and the last level the receiver remembers.
    pub fn read_all() -> Vec<Device> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for path in hid_paths() {
            let Some(info) = open(&path, 0) else { continue };
            let Some((pid, product, len)) = query_collection(&info) else { continue };
            if !seen.insert(pid) {
                continue;
            }
            let Some(rw) = open(&path, GENERIC_READ.0 | GENERIC_WRITE.0) else {
                log::debug!("receiver {VID:04X}:{pid:04X} could not be opened for the query");
                continue;
            };
            let Some(reply) = query(&rw, len) else { continue };
            for super::Slot { kind, level, online } in parse_reply(&reply) {
                let slot = format!("{kind:?}").to_lowercase();
                let name = if product.is_empty() { format!("Receiver 2.4 GHz ({slot})") } else { product.clone() };
                out.push(Device {
                    id: format!("hid:{VID:04x}:{pid:04x}:{slot}"),
                    name,
                    kind,
                    battery: Some(level),
                    connected: online,
                    source: BatterySource::Dongle,
                    details: None,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reply_seen_on_inno_x3_receiver() {
        // Read from receiver 3151:5031 after an F7 query: keyboard slot empty,
        // mouse offline (asleep) with a last known level of 100%.
        let reply = [0x00, 0x00, 0x00, 0x64, 0x01, 0x01, 0x01, 0x02, 0x01, 0x08];
        assert_eq!(
            parse_reply(&reply),
            vec![Slot { kind: DeviceKind::Mouse, level: 100, online: false }]
        );
    }

    #[test]
    fn both_slots_online() {
        let reply = [0x00, 0x01, 0x37, 0x0A, 0x00, 0x00, 0x00];
        assert_eq!(
            parse_reply(&reply),
            vec![
                Slot { kind: DeviceKind::Keyboard, level: 55, online: true },
                Slot { kind: DeviceKind::Mouse, level: 10, online: true },
            ]
        );
    }

    #[test]
    fn rejects_empty_short_or_out_of_range() {
        assert!(parse_reply(&[0; 7]).is_empty());
        assert!(parse_reply(&[0, 0, 50, 50]).is_empty());
        // Levels of 0 or > 100 are ignored.
        assert!(parse_reply(&[0, 0, 0, 0xFF, 1, 0, 0]).is_empty());
        assert!(parse_reply(&[0, 0, 0, 0, 1, 0, 0]).is_empty());
    }

    #[test]
    fn known_pid_list_contains_5031() {
        assert!(PIDS.contains(&0x5031));
    }
}
