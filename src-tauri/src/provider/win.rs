//! Windows provider based on WinRT (the `windows` crate).
//!
//! 1. Enumerate paired Bluetooth devices (classic + LE) as AssociationEndpoints
//!    → name, connection state, ContainerId.
//! 2. Enumerate PnP devnodes with the battery property that Windows Settings also uses
//!    (`{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2`) → map of ContainerId → %.
//! 3. Fallback for connected LE devices without that property:
//!    read the GATT Battery Service (0x180F) / Battery Level (0x2A19).
//! 4. Vendor protocols: Huawei FreeBuds over SPP (`huawei.rs`), 2.4 GHz receivers (`rongyuan.rs`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use windows::core::{IInspectable, Interface, GUID, HSTRING};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids,
};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
use windows::Foundation::{IPropertyValue, PropertyType};
use windows::Storage::Streams::DataReader;
use windows_collections::{IIterable, IMapView};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use super::{huawei, merge, rongyuan, BatteryProvider, RawEntry};
use crate::model::{kind_from_appearance, kind_from_cod, sort_devices, Device, DeviceKind};

/// DEVPKEY battery level Bluetooth (DEVPROP_TYPE_BYTE, 0..=100).
const PKEY_BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
const PKEY_CONTAINER: &str = "System.Devices.ContainerId";
const PKEY_AEP_CONTAINER: &str = "System.Devices.Aep.ContainerId";
const PKEY_AEP_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const AQS_PRESENT: &str = "System.Devices.Present:=System.StructuredQueryType.Boolean#True";

pub fn init_thread() {
    // May fail if the thread is already initialized; WinRT is still usable then.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transport {
    Classic,
    Le,
}

pub struct WindowsProvider {
    /// A device's kind rarely changes; read it once per id.
    kind_cache: HashMap<String, DeviceKind>,
}

impl WindowsProvider {
    pub fn new() -> Self {
        Self {
            kind_cache: HashMap::new(),
        }
    }

    fn kind_for(&mut self, id: &str, transport: Transport) -> DeviceKind {
        if let Some(k) = self.kind_cache.get(id) {
            return *k;
        }
        let kind = match transport {
            Transport::Classic => classic_kind(id),
            Transport::Le => le_kind(id),
        }
        .unwrap_or(DeviceKind::Other);
        self.kind_cache.insert(id.to_owned(), kind);
        kind
    }
}

impl BatteryProvider for WindowsProvider {
    fn scan(&mut self) -> Result<Vec<Device>> {
        let mut entries: Vec<(RawEntry, Transport)> = Vec::new();

        let selectors = [
            (
                Transport::Classic,
                BluetoothDevice::GetDeviceSelectorFromPairingState(true)?,
            ),
            (
                Transport::Le,
                BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?,
            ),
        ];
        for (transport, selector) in selectors {
            let list = find_all(
                &selector,
                &[PKEY_AEP_CONNECTED, PKEY_AEP_CONTAINER],
                DeviceInformationKind::AssociationEndpoint,
            )
            .context("failed to enumerate Bluetooth devices")?;

            for info in list {
                let id = info.Id()?.to_string();
                let name = info.Name()?.to_string();
                let props = info.Properties()?;
                let connected = prop_value(&props, PKEY_AEP_CONNECTED)
                    .and_then(|v| v.GetBoolean().ok())
                    .unwrap_or(false);
                let container = prop_guid(&props, PKEY_AEP_CONTAINER);
                let kind = self.kind_for(&id, transport);
                entries.push((
                    RawEntry {
                        id,
                        name,
                        container,
                        connected,
                        kind,
                        gatt_battery: None,
                    },
                    transport,
                ));
            }
        }

        let pnp = pnp_battery_map().unwrap_or_else(|e| {
            log::warn!("failed to read the PnP battery property: {e:#}");
            HashMap::new()
        });

        for (entry, transport) in entries.iter_mut() {
            let has_pnp = entry.container.is_some_and(|c| pnp.contains_key(&c));
            if *transport == Transport::Le && entry.connected && !has_pnp {
                match read_gatt_battery(&entry.id) {
                    Ok(b) => entry.gatt_battery = b,
                    Err(e) => log::debug!("GATT battery read for {} failed: {e:#}", entry.name),
                }
            }
        }

        let mut devices = merge(entries.into_iter().map(|(e, _)| e).collect(), &pnp);
        // Huawei FreeBuds report only a coarse level over Hands-Free; ask the earbuds directly.
        for d in devices.iter_mut().filter(|d| d.connected && huawei::is_supported_name(&d.name)) {
            match huawei::read_battery(&d.id) {
                Ok(Some(b)) => huawei::apply(d, b),
                Ok(None) => {}
                Err(e) => log::debug!("Huawei SPP battery read for {} failed: {e:#}", d.name),
            }
        }
        // Mice/keyboards connected through a 2.4 GHz receiver (not Bluetooth).
        devices.extend(rongyuan::read_all());
        sort_devices(&mut devices);
        Ok(devices)
    }
}

/// ContainerId → battery level, from every devnode that is currently present.
fn pnp_battery_map() -> Result<HashMap<u128, u8>> {
    let nodes = find_all(
        &HSTRING::from(AQS_PRESENT),
        &[PKEY_BATTERY, PKEY_CONTAINER],
        DeviceInformationKind::Device,
    )?;
    let mut map = HashMap::new();
    for node in nodes {
        let Ok(props) = node.Properties() else { continue };
        let (Some(level), Some(container)) = (prop_u8(&props, PKEY_BATTERY), prop_guid(&props, PKEY_CONTAINER))
        else {
            continue;
        };
        map.entry(container).or_insert(level.min(100));
    }
    Ok(map)
}

fn find_all(aqs: &HSTRING, props: &[&str], kind: DeviceInformationKind) -> Result<Vec<DeviceInformation>> {
    let props: Vec<HSTRING> = props.iter().map(|p| HSTRING::from(*p)).collect();
    let props: IIterable<HSTRING> = props.into();
    let coll =
        DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(aqs, &props, kind)?.get()?;
    let mut out = Vec::with_capacity(coll.Size()? as usize);
    for i in 0..coll.Size()? {
        out.push(coll.GetAt(i)?);
    }
    Ok(out)
}

fn classic_kind(id: &str) -> Result<DeviceKind> {
    let dev = BluetoothDevice::FromIdAsync(&HSTRING::from(id))?.get()?;
    let raw = dev.ClassOfDevice()?.RawValue()?;
    let _ = dev.Close();
    Ok(kind_from_cod(raw))
}

fn le_kind(id: &str) -> Result<DeviceKind> {
    let dev = BluetoothLEDevice::FromIdAsync(&HSTRING::from(id))?.get()?;
    let appearance = dev.Appearance()?;
    let kind = kind_from_appearance(appearance.Category()?, appearance.SubCategory()?);
    let _ = dev.Close();
    Ok(kind)
}

fn read_gatt_battery(id: &str) -> Result<Option<u8>> {
    let dev = BluetoothLEDevice::FromIdAsync(&HSTRING::from(id))?.get()?;
    let result = (|| -> Result<Option<u8>> {
        let services = dev
            .GetGattServicesForUuidWithCacheModeAsync(GattServiceUuids::Battery()?, BluetoothCacheMode::Cached)?
            .get()?;
        if services.Status()? != GattCommunicationStatus::Success {
            return Ok(None);
        }
        let services = services.Services()?;
        if services.Size()? == 0 {
            return Ok(None);
        }
        let service = services.GetAt(0)?;
        let read = (|| -> Result<Option<u8>> {
            let chars = service
                .GetCharacteristicsForUuidWithCacheModeAsync(
                    GattCharacteristicUuids::BatteryLevel()?,
                    BluetoothCacheMode::Cached,
                )?
                .get()?;
            if chars.Status()? != GattCommunicationStatus::Success {
                return Ok(None);
            }
            let chars = chars.Characteristics()?;
            if chars.Size()? == 0 {
                return Ok(None);
            }
            let value = chars
                .GetAt(0)?
                .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?
                .get()?;
            if value.Status()? != GattCommunicationStatus::Success {
                return Ok(None);
            }
            let reader = DataReader::FromBuffer(&value.Value()?)?;
            Ok(Some(reader.ReadByte()?.min(100)))
        })();
        let _ = service.Close();
        read
    })();
    let _ = dev.Close();
    result
}

// ---- property helpers ------------------------------------------------------

fn prop_value(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<IPropertyValue> {
    let hkey = HSTRING::from(key);
    let value = if props.HasKey(&hkey).ok()? {
        props.Lookup(&hkey).ok()?
    } else {
        // In case Windows normalizes the key spelling (upper/lower case).
        let it = props.First().ok()?;
        let mut found = None;
        while it.HasCurrent().unwrap_or(false) {
            if let Ok(pair) = it.Current() {
                if pair.Key().ok()?.to_string().eq_ignore_ascii_case(key) {
                    found = pair.Value().ok();
                    break;
                }
            }
            if !it.MoveNext().unwrap_or(false) {
                break;
            }
        }
        found?
    };
    value.cast::<IPropertyValue>().ok()
}

fn prop_guid(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<u128> {
    let g: GUID = prop_value(props, key)?.GetGuid().ok()?;
    Some(g.to_u128())
}

fn prop_u8(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<u8> {
    let v = prop_value(props, key)?;
    let n: i64 = match v.Type().ok()? {
        PropertyType::UInt8 => v.GetUInt8().ok()?.into(),
        PropertyType::UInt16 => v.GetUInt16().ok()?.into(),
        PropertyType::UInt32 => v.GetUInt32().ok()?.into(),
        PropertyType::Int16 => v.GetInt16().ok()?.into(),
        PropertyType::Int32 => v.GetInt32().ok()?.into(),
        _ => return None,
    };
    u8::try_from(n.clamp(0, 100)).ok()
}
