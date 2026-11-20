//! Debug: show what HID devices expose (e.g. a 2.4 GHz mouse dongle), especially battery
//! usages. Read-only (GET_REPORT); never writes anything to the device.
//!   cargo run --example hidprobe            → summary of every HID collection
//!   cargo run --example hidprobe -- 1234    → details + try reading reports for VID 0x1234

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
    };
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let filter_vid: Option<u16> = std::env::args()
        .nth(1)
        .and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok());

    let guid: GUID = unsafe { HidD_GetHidGuid() };
    let paths: Vec<String> = unsafe {
        let mut len = 0u32;
        if CM_Get_Device_Interface_List_SizeW(&mut len, &guid, PCWSTR::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT)
            != CR_SUCCESS
        {
            eprintln!("failed to get the HID interface list");
            return;
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_Interface_ListW(&guid, PCWSTR::null(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT)
            != CR_SUCCESS
        {
            eprintln!("failed to get the HID interface list");
            return;
        }
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    };

    let open = |path: &str, access: u32| -> Option<HANDLE> {
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
        }
    };

    let page_name = |p: u16| match p {
        0x01 => "GenericDesktop",
        0x06 => "GenericDevice",
        0x07 => "Keyboard",
        0x0C => "Consumer",
        0x84 => "PowerDevice",
        0x85 => "BatterySystem",
        p if p >= 0xFF00 => "Vendor",
        _ => "Other",
    };

    println!("{} HID interfaces\n", paths.len());
    for path in &paths {
        let Some(h) = open(path, 0) else { continue };
        unsafe {
            let mut attrs = HIDD_ATTRIBUTES {
                Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                ..Default::default()
            };
            let _ = HidD_GetAttributes(h, &mut attrs);
            if filter_vid.is_some_and(|v| v != attrs.VendorID) {
                let _ = CloseHandle(h);
                continue;
            }
            let mut name = [0u16; 128];
            let product = if HidD_GetProductString(h, name.as_mut_ptr().cast(), 256) {
                String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(0)])
            } else {
                "?".into()
            };
            let mut pp = PHIDP_PREPARSED_DATA::default();
            if !HidD_GetPreparsedData(h, &mut pp) {
                let _ = CloseHandle(h);
                continue;
            }
            let mut caps = HIDP_CAPS::default();
            let _ = HidP_GetCaps(pp, &mut caps);
            println!(
                "VID {:04X} PID {:04X}  {:<28} top={:#06X}:{:#04X} ({})  in={} feat={}",
                attrs.VendorID,
                attrs.ProductID,
                product,
                caps.UsagePage,
                caps.Usage,
                page_name(caps.UsagePage),
                caps.InputReportByteLength,
                caps.FeatureReportByteLength,
            );

            let mut interesting = Vec::new();
            for (kind, n) in [(HidP_Input, caps.NumberInputValueCaps), (HidP_Feature, caps.NumberFeatureValueCaps)] {
                if n == 0 {
                    continue;
                }
                let mut vc = vec![HIDP_VALUE_CAPS::default(); n as usize];
                let mut len = n;
                if HidP_GetValueCaps(kind, vc.as_mut_ptr(), &mut len, pp) != HIDP_STATUS_SUCCESS {
                    continue;
                }
                for c in &vc[..len as usize] {
                    let usage = if c.IsRange { c.Anonymous.Range.UsageMin } else { c.Anonymous.NotRange.Usage };
                    let battery = (c.UsagePage == 0x06 && usage == 0x20) || matches!(c.UsagePage, 0x84 | 0x85);
                    if battery || filter_vid.is_some() {
                        println!(
                            "    {} {:>7} page={:#06X} usage={:#04X} id={} bits={}x{} range={}..{}{}",
                            if battery { "★" } else { " " },
                            if kind == HidP_Input { "input" } else { "feature" },
                            c.UsagePage,
                            usage,
                            c.ReportID,
                            c.BitSize,
                            c.ReportCount,
                            c.LogicalMin,
                            c.LogicalMax,
                            if battery { "  ← battery candidate" } else { "" },
                        );
                    }
                    if battery {
                        interesting.push((kind, c.UsagePage, usage, c.ReportID, c.LinkCollection));
                    }
                }
            }

            // Detail mode: try reading reports (GET_REPORT) from non-standard collections.
            let readable = caps.UsagePage >= 0xFF00 || !interesting.is_empty();
            if filter_vid.is_some() && readable {
                if let Some(rw) = open(path, GENERIC_READ.0 | GENERIC_WRITE.0).or_else(|| open(path, GENERIC_READ.0)) {
                    for (kind, page, usage, id, link) in &interesting {
                        let size = if *kind == HidP_Input { caps.InputReportByteLength } else { caps.FeatureReportByteLength };
                        let mut report = vec![0u8; size as usize];
                        report[0] = *id;
                        let ok = if *kind == HidP_Input {
                            HidD_GetInputReport(rw, report.as_mut_ptr().cast(), size as u32)
                        } else {
                            HidD_GetFeature(rw, report.as_mut_ptr().cast(), size as u32)
                        };
                        let mut value = 0u32;
                        let st = HidP_GetUsageValue(*kind, *page, Some(*link), *usage, &mut value, pp, &report);
                        println!("    read report {id}: ok={ok} value={} raw={:02X?}", if st == HIDP_STATUS_SUCCESS { value.to_string() } else { "?".into() }, report);
                    }
                    if caps.UsagePage >= 0xFF00 && caps.FeatureReportByteLength > 0 {
                        for id in 0u8..=16 {
                            let mut report = vec![0u8; caps.FeatureReportByteLength as usize];
                            report[0] = id;
                            if HidD_GetFeature(rw, report.as_mut_ptr().cast(), report.len() as u32) {
                                println!("    feature report {id}: {:02X?}", report);
                            }
                        }
                    }
                    let _ = CloseHandle(rw);
                } else {
                    println!("    (could not be opened for reading)");
                }
            }

            let _ = HidD_FreePreparsedData(pp);
            let _ = CloseHandle(h);
        }
    }
}
