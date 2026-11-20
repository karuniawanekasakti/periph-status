//! Huawei FreeBuds proprietary SPP protocol (RFCOMM), used here to read precise battery levels
//! (global + left/right/case) instead of the coarse value Windows gets over Hands-Free.
//!
//! Frame layout (all multi-byte fields big-endian):
//!
//! ```text
//! 5A | len:u16 | 00 | cmd:[u8; 2] | TLV… | crc16:u16
//! len = 2 (cmd) + TLV bytes + 1;  TLV = type:u8, length:u8, value
//! crc = CRC-16/XMODEM over every byte before it (including the 5A header)
//! ```
//!
//! Battery: request `01 08` with empty TLVs 1, 2, 3; reply (or unsolicited `01 27`) carries
//! TLV 1 = global %, TLV 2 = [left %, right %, case %], TLV 3 = charging flags.
//!
//! Wire format as documented by the OpenFreebuds project (melianmiko/OpenFreebuds) and
//! hrbtnog/freebuds-client (`docs/PROTOCOL.md`). This is an independent implementation.

use crate::model::{BatterySource, Device, TwsBattery};

pub const MAGIC: u8 = 0x5A;
pub const CMD_BATTERY_READ: [u8; 2] = [0x01, 0x08];
pub const CMD_BATTERY_NOTIFY: [u8; 2] = [0x01, 0x27];

/// CRC-16/XMODEM: polynomial 0x1021, init 0, no reflection, no final xor.
pub fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// Build a frame for `cmd` with the given TLV parameters.
pub fn encode(cmd: [u8; 2], params: &[(u8, &[u8])]) -> Vec<u8> {
    let mut body = cmd.to_vec();
    for (kind, value) in params {
        body.push(*kind);
        body.push(value.len() as u8);
        body.extend_from_slice(value);
    }
    let len = (body.len() + 1) as u16;
    let mut frame = vec![MAGIC];
    frame.extend_from_slice(&len.to_be_bytes());
    frame.push(0x00);
    frame.extend_from_slice(&body);
    let crc = crc16_xmodem(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

/// Battery read request: TLVs 1, 2 and 3 with empty values ("please fill in").
pub fn battery_request() -> Vec<u8> {
    encode(CMD_BATTERY_READ, &[(1, &[]), (2, &[]), (3, &[])])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub cmd: [u8; 2],
    pub params: Vec<(u8, Vec<u8>)>,
}

impl Packet {
    pub fn param(&self, kind: u8) -> Option<&[u8]> {
        self.params.iter().find(|(k, _)| *k == kind).map(|(_, v)| v.as_slice())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// A complete, valid frame and the number of bytes it used.
    Packet(Packet, usize),
    /// Need more bytes.
    Incomplete,
    /// Not a valid frame at the start of the buffer; skip this many bytes and retry.
    Invalid(usize),
}

/// Decode one frame from the start of `buf` (stream data may be fragmented or concatenated).
pub fn decode(buf: &[u8]) -> Decoded {
    if buf.is_empty() {
        return Decoded::Incomplete;
    }
    if buf[0] != MAGIC {
        return Decoded::Invalid(1);
    }
    if buf.len() < 4 {
        return Decoded::Incomplete;
    }
    let len = usize::from(u16::from_be_bytes([buf[1], buf[2]]));
    // len counts cmd + TLVs + 1; the frame adds magic, length, the 00 byte and the CRC.
    if len < 3 || buf[3] != 0 {
        return Decoded::Invalid(1);
    }
    let total = len + 5;
    if buf.len() < total {
        return Decoded::Incomplete;
    }
    let crc = u16::from_be_bytes([buf[total - 2], buf[total - 1]]);
    if crc16_xmodem(&buf[..total - 2]) != crc {
        return Decoded::Invalid(1);
    }

    let cmd = [buf[4], buf[5]];
    let tlv = &buf[6..total - 2];
    let mut params = Vec::new();
    let mut pos = 0;
    while pos + 2 <= tlv.len() {
        let kind = tlv[pos];
        let size = usize::from(tlv[pos + 1]);
        let end = (pos + 2 + size).min(tlv.len());
        params.push((kind, tlv[pos + 2..end].to_vec()));
        pos = pos + 2 + size;
    }
    Decoded::Packet(Packet { cmd, params }, total)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Battery {
    pub global: Option<u8>,
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case: Option<u8>,
    pub charging: bool,
}

/// Extract battery levels from a battery reply or notification.
pub fn parse_battery(packet: &Packet) -> Option<Battery> {
    if packet.cmd != CMD_BATTERY_READ && packet.cmd != CMD_BATTERY_NOTIFY {
        return None;
    }
    let level = |b: u8| (b <= 100).then_some(b);
    let mut out = Battery::default();
    if let Some(&[g]) = packet.param(1) {
        out.global = level(g);
    }
    if let Some(&[l, r, c]) = packet.param(2) {
        out.left = level(l);
        out.right = level(r);
        out.case = level(c);
    }
    if let Some(flags) = packet.param(3) {
        out.charging = flags.contains(&1);
    }
    (out.global.is_some() || out.left.is_some() || out.right.is_some()).then_some(out)
}

/// Whether a paired device looks like Huawei earbuds that speak this protocol.
pub fn is_supported_name(name: &str) -> bool {
    name.to_lowercase().contains("freebuds")
}

/// Apply precise levels to a device: the global level becomes the main battery value
/// (falling back to the lower earbud), and the per-part levels go into `details`.
pub fn apply(device: &mut Device, b: Battery) {
    let lowest_bud = match (b.left, b.right) {
        (Some(l), Some(r)) => Some(l.min(r)),
        (l, r) => l.or(r),
    };
    device.battery = b.global.or(lowest_bud);
    device.source = BatterySource::Vendor;
    device.details = Some(TwsBattery { left: b.left, right: b.right, case: b.case, charging: b.charging });
}

#[cfg(windows)]
pub use win::read_battery;

#[cfg(windows)]
mod win {
    use std::time::{Duration, Instant};

    use anyhow::{bail, Context, Result};
    use windows::core::HSTRING;
    use windows::Devices::Bluetooth::Rfcomm::RfcommServiceId;
    use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothDevice};
    use windows::Networking::Sockets::StreamSocket;
    use windows::Storage::Streams::{DataReader, DataWriter, InputStreamOptions};
    use windows_future::AsyncStatus;

    use super::{battery_request, decode, parse_battery, Battery, Decoded};

    const READ_TIMEOUT: Duration = Duration::from_secs(3);

    /// Connect to the device's Serial Port service, send a battery request and wait for the reply.
    /// `device_id` is the Bluetooth (classic) device id from enumeration.
    pub fn read_battery(device_id: &str) -> Result<Option<Battery>> {
        let dev = BluetoothDevice::FromIdAsync(&HSTRING::from(device_id))?.get()?;
        let spp = RfcommServiceId::SerialPort()?.Uuid()?;
        let mut service = None;
        // Cached SDP results are fast; only fall back to a live SDP query when needed.
        for mode in [BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached] {
            let services = dev.GetRfcommServicesWithCacheModeAsync(mode)?.get()?.Services()?;
            for i in 0..services.Size()? {
                let svc = services.GetAt(i)?;
                if svc.ServiceId()?.Uuid()? == spp {
                    service = Some(svc);
                    break;
                }
            }
            if service.is_some() {
                break;
            }
        }
        let _ = dev.Close();
        let Some(service) = service else { return Ok(None) };

        let socket = StreamSocket::new()?;
        let result = (|| -> Result<Option<Battery>> {
            socket
                .ConnectAsync(&service.ConnectionHostName()?, &service.ConnectionServiceName()?)?
                .get()
                .context("RFCOMM connect failed")?;
            let writer = DataWriter::CreateDataWriter(&socket.OutputStream()?)?;
            writer.WriteBytes(&battery_request())?;
            writer.StoreAsync()?.get()?;

            let reader = DataReader::CreateDataReader(&socket.InputStream()?)?;
            reader.SetInputStreamOptions(InputStreamOptions::Partial)?;
            let deadline = Instant::now() + READ_TIMEOUT;
            let mut buf = Vec::new();
            while Instant::now() < deadline {
                let op = reader.LoadAsync(1024)?;
                while op.Status()? == AsyncStatus::Started && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if op.Status()? != AsyncStatus::Completed {
                    let _ = op.Cancel();
                    bail!("timed out waiting for the battery reply");
                }
                let n = op.GetResults()? as usize;
                if n == 0 {
                    bail!("connection closed by the device");
                }
                let mut chunk = vec![0u8; n];
                reader.ReadBytes(&mut chunk)?;
                buf.extend_from_slice(&chunk);
                loop {
                    match decode(&buf) {
                        Decoded::Packet(packet, used) => {
                            buf.drain(..used);
                            if let Some(battery) = parse_battery(&packet) {
                                return Ok(Some(battery));
                            }
                        }
                        Decoded::Invalid(skip) => {
                            buf.drain(..skip);
                        }
                        Decoded::Incomplete => break,
                    }
                }
            }
            bail!("timed out waiting for the battery reply")
        })();
        let _ = socket.Close();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_documented_vector() {
        // hrbtnog/freebuds-client docs/PROTOCOL.md: 5A 00 06 00 2B 03 08 01 00 → A7 9D
        assert_eq!(crc16_xmodem(&[0x5A, 0x00, 0x06, 0x00, 0x2B, 0x03, 0x08, 0x01, 0x00]), 0xA79D);
    }

    #[test]
    fn battery_request_bytes() {
        assert_eq!(
            battery_request(),
            [0x5A, 0x00, 0x09, 0x00, 0x01, 0x08, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0xFB, 0xB9]
        );
    }

    const REPLY: [u8; 32] = [
        0x5A, 0x00, 0x1B, 0x00, 0x01, 0x08, 0x01, 0x01, 0x53, 0x02, 0x03, 0x53, 0x53, 0x45, 0x03, 0x03,
        0x01, 0x01, 0x01, 0x04, 0x02, 0x0A, 0x14, 0x05, 0x02, 0x01, 0x01, 0x06, 0x01, 0x0A, 0x9F, 0x73,
    ];

    #[test]
    fn decodes_documented_reply() {
        // Captured FreeBuds Pro 5 reply from the same document.
        let Decoded::Packet(p, used) = decode(&REPLY) else { panic!("not decoded") };
        assert_eq!(used, 32);
        assert_eq!(
            parse_battery(&p),
            Some(Battery { global: Some(83), left: Some(83), right: Some(83), case: Some(69), charging: true })
        );
    }

    #[test]
    fn handles_fragments_garbage_and_bad_crc() {
        assert_eq!(decode(&REPLY[..10]), Decoded::Incomplete);
        assert_eq!(decode(&[0x00, 0x5A]), Decoded::Invalid(1));
        let mut bad = REPLY;
        bad[31] ^= 0xFF;
        assert_eq!(decode(&bad), Decoded::Invalid(1));
        // Two frames back to back: the first one is consumed exactly.
        let mut two = REPLY.to_vec();
        two.extend_from_slice(&battery_request());
        assert!(matches!(decode(&two), Decoded::Packet(_, 32)));
    }

    #[test]
    fn apply_sets_global_and_details() {
        let mut d = Device {
            id: "x".into(),
            name: "HUAWEI FreeBuds 6i".into(),
            kind: crate::model::DeviceKind::Headset,
            battery: Some(100),
            connected: true,
            source: BatterySource::Pnp,
            details: None,
        };
        // Values read from a FreeBuds 6i on Windows.
        apply(&mut d, Battery { global: Some(91), left: Some(93), right: Some(91), case: Some(100), charging: false });
        assert_eq!(d.battery, Some(91));
        assert_eq!(d.source, BatterySource::Vendor);
        assert_eq!(d.details.unwrap().case, Some(100));
        // Without a global value, use the lower earbud.
        apply(&mut d, Battery { global: None, left: Some(40), right: Some(35), case: None, charging: true });
        assert_eq!(d.battery, Some(35));
        assert!(is_supported_name("HUAWEI FreeBuds 6i"));
        assert!(!is_supported_name("Galaxy Buds2"));
    }

    #[test]
    fn decodes_freebuds_6i_reply() {
        // Reply captured from a FreeBuds 6i: global 91, left 93, right 91, case 100.
        let reply = [
            0x5A, 0x00, 0x1B, 0x00, 0x01, 0x08, 0x01, 0x01, 0x5B, 0x02, 0x03, 0x5D, 0x5B, 0x64, 0x03, 0x03,
            0x00, 0x00, 0x00, 0x04, 0x02, 0x14, 0x0A, 0x05, 0x02, 0x00, 0x00, 0x06, 0x01, 0x0A, 0x0A, 0xDF,
        ];
        let Decoded::Packet(p, _) = decode(&reply) else { panic!("not decoded") };
        assert_eq!(
            parse_battery(&p),
            Some(Battery { global: Some(91), left: Some(93), right: Some(91), case: Some(100), charging: false })
        );
    }

    #[test]
    fn ignores_other_commands() {
        let Decoded::Packet(p, _) = decode(&encode([0x01, 0x07], &[(1, &[50])])) else { panic!() };
        assert_eq!(parse_battery(&p), None);
    }
}
