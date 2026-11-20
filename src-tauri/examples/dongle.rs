//! Debug: query a Rongyuan receiver (VID 3151) status step by step.
//!   cargo run --example dongle

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    use std::time::Duration;

    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
    };
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::Foundation::{GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let open = |path: &str, access: u32| -> windows::core::Result<HANDLE> {
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
        }
    };

    let paths: Vec<String> = unsafe {
        let guid: GUID = HidD_GetHidGuid();
        let mut len = 0u32;
        let _ = CM_Get_Device_Interface_List_SizeW(&mut len, &guid, PCWSTR::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT);
        let mut buf = vec![0u16; len as usize];
        let _ = CM_Get_Device_Interface_ListW(&guid, PCWSTR::null(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT);
        buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    };

    for path in paths {
        let Ok(h) = open(&path, 0) else { continue };
        unsafe {
            let mut attrs = HIDD_ATTRIBUTES {
                Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                ..Default::default()
            };
            let _ = HidD_GetAttributes(h, &mut attrs);
            if attrs.VendorID != 0x3151 {
                continue;
            }
            let mut pp = PHIDP_PREPARSED_DATA::default();
            let _ = HidD_GetPreparsedData(h, &mut pp);
            let mut caps = HIDP_CAPS::default();
            let _ = HidP_GetCaps(pp, &mut caps);
            if caps.UsagePage != 0xFFFF || caps.Usage != 2 {
                continue;
            }
            println!("collection: {path}");
            println!("  feature len = {}", caps.FeatureReportByteLength);
            let len = caps.FeatureReportByteLength as usize;
            let rw = match open(&path, GENERIC_READ.0 | GENERIC_WRITE.0) {
                Ok(rw) => rw,
                Err(e) => {
                    println!("  opening read/write failed: {e}");
                    continue;
                }
            };
            let mut cmd = vec![0u8; len];
            cmd[1] = 0xF7;
            let ok = HidD_SetFeature(rw, cmd.as_ptr().cast(), len as u32);
            println!("  SET_FEATURE [00 F7 …] ok={ok} lasterr={:?}", if ok { None } else { Some(GetLastError()) });
            for wait in [20u64, 100, 300] {
                std::thread::sleep(Duration::from_millis(wait));
                let mut reply = vec![0u8; len];
                let ok = HidD_GetFeature(rw, reply.as_mut_ptr().cast(), len as u32);
                println!("  GET_FEATURE (+{wait} ms) ok={ok}: {:02X?}", &reply[..16.min(len)]);
            }
        }
    }
}
