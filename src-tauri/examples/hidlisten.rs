//! Passive debugging for HID dongles: listen to input reports from vendor collections and
//! re-read the feature report periodically, printing the bytes that change.
//! Never sends any command to the device.
//!   cargo run --example hidlisten -- 3151 60      (VID in hex, duration in seconds)

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};

    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
    };
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let mut args = std::env::args().skip(1);
    let vid = args
        .next()
        .and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x3151);
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    let guid: GUID = unsafe { HidD_GetHidGuid() };
    let paths: Vec<String> = unsafe {
        let mut len = 0u32;
        assert_eq!(
            CM_Get_Device_Interface_List_SizeW(&mut len, &guid, PCWSTR::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT),
            CR_SUCCESS
        );
        let mut buf = vec![0u16; len as usize];
        assert_eq!(
            CM_Get_Device_Interface_ListW(&guid, PCWSTR::null(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT),
            CR_SUCCESS
        );
        buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
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

    // Find the vendor collections that belong to this VID.
    let mut input_paths = Vec::new(); // (path, input report length)
    let mut feature_path = None; // (path, feature report length)
    for path in &paths {
        let Some(h) = open(path, 0) else { continue };
        unsafe {
            let mut attrs = HIDD_ATTRIBUTES {
                Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                ..Default::default()
            };
            let _ = HidD_GetAttributes(h, &mut attrs);
            let mut pp = PHIDP_PREPARSED_DATA::default();
            if attrs.VendorID == vid && HidD_GetPreparsedData(h, &mut pp) {
                let mut caps = HIDP_CAPS::default();
                let _ = HidP_GetCaps(pp, &mut caps);
                if caps.UsagePage >= 0xFF00 {
                    if caps.InputReportByteLength > 0 {
                        input_paths.push((path.clone(), caps.InputReportByteLength as usize));
                    }
                    if caps.FeatureReportByteLength > 0 {
                        feature_path = Some((path.clone(), caps.FeatureReportByteLength as usize));
                    }
                }
                let _ = HidD_FreePreparsedData(pp);
            }
            let _ = CloseHandle(h);
        }
    }
    println!(
        "VID {vid:04X}: {} vendor input collection(s), feature={}",
        input_paths.len(),
        feature_path.is_some()
    );

    let start = Instant::now();
    let stamp = move || format!("{:>6.1}s", start.elapsed().as_secs_f32());

    // Input report reader thread (blocking ReadFile).
    for (path, len) in input_paths {
        let Some(h) = open(&path, GENERIC_READ.0 | GENERIC_WRITE.0).or_else(|| open(&path, GENERIC_READ.0)) else {
            println!("could not open the vendor input collection");
            continue;
        };
        let raw = h.0 as isize;
        std::thread::spawn(move || {
            let h = HANDLE(raw as *mut _);
            let mut buf = vec![0u8; len];
            loop {
                let mut read = 0u32;
                if unsafe { ReadFile(h, Some(&mut buf), Some(&mut read), None) }.is_err() {
                    println!("{} ReadFile failed, stopping", stamp());
                    break;
                }
                println!("{} INPUT  {:02X?}", stamp(), &buf[..read as usize]);
            }
        });
    }

    // Re-read the feature report every 2 seconds and print it when it changes.
    let feature = feature_path.and_then(|(path, len)| {
        open(&path, GENERIC_READ.0 | GENERIC_WRITE.0)
            .or_else(|| open(&path, GENERIC_READ.0))
            .map(|h| (h, len))
    });
    let mut last: Option<Vec<u8>> = None;
    println!("listening for {secs} seconds… (move/click the mouse, or switch it off and on)");
    while start.elapsed() < Duration::from_secs(secs) {
        if let Some((h, len)) = feature {
            let mut report = vec![0u8; len];
            if unsafe { HidD_GetFeature(h, report.as_mut_ptr().cast(), len as u32) } {
                match &last {
                    None => println!("{} FEAT   {:02X?}", stamp(), report),
                    Some(prev) if *prev != report => {
                        let diff: Vec<String> = prev
                            .iter()
                            .zip(&report)
                            .enumerate()
                            .filter(|(_, (a, b))| a != b)
                            .map(|(i, (a, b))| format!("[{i}] {a:02X}→{b:02X}"))
                            .collect();
                        println!("{} FEAT changed: {}", stamp(), diff.join(", "));
                    }
                    _ => {}
                }
                last = Some(report);
            } else if last.is_some() {
                println!("{} FEAT read failed (mouse off / asleep?)", stamp());
                last = None;
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    println!("{} done", stamp());
    std::process::exit(0);
}
