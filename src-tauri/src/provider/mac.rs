//! macOS placeholder. Plan: IOKit (`BatteryPercent` in the IOKit registry for Apple
//! devices such as Magic Mouse/Keyboard/AirPods) + CoreBluetooth GATT Battery Service
//! for other devices.

use super::BatteryProvider;
use crate::model::Device;

pub struct MacProvider;

impl BatteryProvider for MacProvider {
    fn scan(&mut self) -> anyhow::Result<Vec<Device>> {
        Ok(Vec::new())
    }
}
