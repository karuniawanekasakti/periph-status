//! Debug: read the battery from Huawei FreeBuds over the proprietary SPP protocol.
//!   cargo run --example huawei              (first paired device whose name contains "FreeBuds")
//!   cargo run --example huawei -- "Buds 6i" (name filter)

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    use anyhow::{bail, Context};
    use periph_status_lib::provider::huawei::{self, Decoded};
    use windows::Devices::Bluetooth::Rfcomm::RfcommServiceId;
    use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothDevice};
    use windows::Devices::Enumeration::DeviceInformation;
    use windows_future::AsyncStatus;
    use windows::Networking::Sockets::StreamSocket;
    use windows::Storage::Streams::{DataReader, DataWriter, InputStreamOptions};

    let filter = std::env::args().nth(1).unwrap_or_else(|| "FreeBuds".into()).to_lowercase();

    let selector = BluetoothDevice::GetDeviceSelectorFromPairingState(true)?;
    let infos = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.get()?;
    let mut found = None;
    for i in 0..infos.Size()? {
        let info = infos.GetAt(i)?;
        if info.Name()?.to_string().to_lowercase().contains(&filter) {
            found = Some(info);
            break;
        }
    }
    let info = found.with_context(|| format!("no paired classic device matching {filter:?}"))?;
    let dev = BluetoothDevice::FromIdAsync(&info.Id()?)?.get()?;
    println!("device: {}  status={:?}", dev.Name()?, dev.ConnectionStatus()?);

    let result = dev.GetRfcommServicesWithCacheModeAsync(BluetoothCacheMode::Uncached)?.get()?;
    println!("RFCOMM services (error={:?}):", result.Error()?);
    let services = result.Services()?;
    let spp = RfcommServiceId::SerialPort()?.Uuid()?;
    let mut chosen = None;
    for i in 0..services.Size()? {
        let svc = services.GetAt(i)?;
        let uuid = svc.ServiceId()?.Uuid()?;
        println!("  {:?}  service={}", uuid, svc.ConnectionServiceName()?);
        if uuid == spp && chosen.is_none() {
            chosen = Some(svc);
        }
    }
    let Some(svc) = chosen else { bail!("no Serial Port (SPP) service found") };

    let socket = StreamSocket::new()?;
    let started = Instant::now();
    socket
        .ConnectAsync(&svc.ConnectionHostName()?, &svc.ConnectionServiceName()?)?
        .get()
        .context("RFCOMM connect failed")?;
    println!("connected in {} ms", started.elapsed().as_millis());

    let writer = DataWriter::CreateDataWriter(&socket.OutputStream()?)?;
    let reader = DataReader::CreateDataReader(&socket.InputStream()?)?;
    reader.SetInputStreamOptions(InputStreamOptions::Partial)?;

    let request = huawei::battery_request();
    println!("TX {:02X?}", request);
    writer.WriteBytes(&request)?;
    writer.StoreAsync()?.get()?;

    // Read for a few seconds (replies and unsolicited notifications), with a timeout per read.
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut buf: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        let op = reader.LoadAsync(1024)?;
        while op.Status()? == AsyncStatus::Started && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if op.Status()? != AsyncStatus::Completed {
            let _ = op.Cancel();
            break;
        }
        let n = op.GetResults()? as usize;
        if n == 0 {
            println!("connection closed by the device");
            break;
        }
        let mut chunk = vec![0u8; n];
        reader.ReadBytes(&mut chunk)?;
        println!("RX {:02X?}", chunk);
        buf.extend_from_slice(&chunk);
        loop {
            match huawei::decode(&buf) {
                Decoded::Packet(p, used) => {
                    buf.drain(..used);
                    println!("  packet cmd={:02X?} params={:02X?}", p.cmd, p.params);
                    if let Some(b) = huawei::parse_battery(&p) {
                        println!("  BATTERY {b:?}");
                    }
                }
                Decoded::Invalid(skip) => {
                    buf.drain(..skip);
                }
                Decoded::Incomplete => break,
            }
        }
    }
    let _ = socket.Close();
    Ok(())
}
