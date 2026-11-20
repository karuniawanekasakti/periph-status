//! Debug: print the raw Bluetooth device properties reported by Windows.
//!   cargo run --example dump

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::core::{IInspectable, Interface, HSTRING};
    use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
    use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
    use windows::Foundation::{IPropertyValue, PropertyType};
    use windows_collections::{IIterable, IMapView};

    fn show(v: &IInspectable) -> String {
        let Ok(p) = v.cast::<IPropertyValue>() else {
            return "<obj>".into();
        };
        let t = p.Type().unwrap_or(PropertyType::Empty);
        match t {
            PropertyType::Boolean => format!("{:?}", p.GetBoolean()),
            PropertyType::UInt8 => format!("u8 {:?}", p.GetUInt8()),
            PropertyType::UInt32 => format!("u32 {:?}", p.GetUInt32()),
            PropertyType::Int32 => format!("i32 {:?}", p.GetInt32()),
            PropertyType::Guid => format!("{:?}", p.GetGuid()),
            PropertyType::String => format!("{:?}", p.GetString()),
            other => format!("<{other:?}>"),
        }
    }

    fn dump_props(props: &IMapView<HSTRING, IInspectable>) {
        let it = props.First().unwrap();
        while it.HasCurrent().unwrap_or(false) {
            let pair = it.Current().unwrap();
            let k = pair.Key().unwrap();
            let v = pair.Value().map(|v| show(&v)).unwrap_or_else(|_| "null".into());
            println!("    {k} = {v}");
            if !it.MoveNext().unwrap_or(false) {
                break;
            }
        }
    }

    let aep_props: Vec<HSTRING> = [
        "System.Devices.Aep.IsConnected",
        "System.Devices.Aep.IsPresent",
        "System.Devices.Aep.ContainerId",
        "System.Devices.Aep.DeviceAddress",
        "System.Devices.Aep.ProtocolId",
    ]
    .iter()
    .map(|s| HSTRING::from(*s))
    .collect();
    let aep_props: IIterable<HSTRING> = aep_props.into();

    for (label, sel, le) in [
        ("CLASSIC", BluetoothDevice::GetDeviceSelectorFromPairingState(true)?, false),
        ("LE", BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?, true),
    ] {
        println!("== {label} AEP ==");
        let coll = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
            &sel,
            &aep_props,
            DeviceInformationKind::AssociationEndpoint,
        )?
        .get()?;
        for i in 0..coll.Size()? {
            let d = coll.GetAt(i)?;
            println!("  {}  [{}]", d.Name()?, d.Id()?);
            dump_props(&d.Properties()?);
            let id = d.Id()?;
            let status = if le {
                BluetoothLEDevice::FromIdAsync(&id)
                    .and_then(|op| op.get())
                    .and_then(|dev| dev.ConnectionStatus())
                    .map(|s| format!("{s:?}"))
            } else {
                BluetoothDevice::FromIdAsync(&id)
                    .and_then(|op| op.get())
                    .and_then(|dev| dev.ConnectionStatus())
                    .map(|s| format!("{s:?}"))
            };
            println!("    ConnectionStatus() = {status:?}");
        }
    }

    println!("== PnP devnodes with the battery property ==");
    let pnp_props: Vec<HSTRING> = [
        "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2",
        "System.Devices.ContainerId",
        "System.Devices.Present",
    ]
    .iter()
    .map(|s| HSTRING::from(*s))
    .collect();
    let pnp_props: IIterable<HSTRING> = pnp_props.into();
    let coll = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &HSTRING::new(),
        &pnp_props,
        DeviceInformationKind::Device,
    )?
    .get()?;
    for i in 0..coll.Size()? {
        let d = coll.GetAt(i)?;
        let props = d.Properties()?;
        let key = HSTRING::from("{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2");
        let has = props.HasKey(&key).unwrap_or(false)
            && props.Lookup(&key).map(|v| v.cast::<IPropertyValue>().is_ok()).unwrap_or(false);
        if has {
            println!("  {}  [{}]", d.Name()?, d.Id()?);
            dump_props(&props);
        }
    }
    Ok(())
}
