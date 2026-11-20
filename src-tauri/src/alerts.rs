//! Pure logic (no Tauri): when to send a low-battery notification, and the tray tooltip text.

use crate::model::Device;

/// A new notification may fire only after the battery has risen to threshold + HYSTERESIS.
pub const HYSTERESIS: u8 = 5;
/// Maximum tray tooltip length on Windows (128 characters including the terminating null).
pub const TOOLTIP_MAX: usize = 127;

/// Notification logic with hysteresis.
///
/// Returns `(send_notification, new_alerted_state)`:
/// - drops to <= threshold and not yet notified → notify once;
/// - rises to >= threshold + HYSTERESIS → reset so it can notify again later;
/// - anything in between / no data → state unchanged.
pub fn evaluate_low(alerted: bool, level: Option<u8>, threshold: u8) -> (bool, bool) {
    match level {
        Some(l) if l <= threshold => (!alerted, true),
        Some(l) if l >= threshold.saturating_add(HYSTERESIS) => (false, false),
        _ => (false, alerted),
    }
}

pub fn tooltip_text(devices: &[Device]) -> String {
    let lines: Vec<String> = devices
        .iter()
        .filter(|d| d.connected)
        .map(|d| match d.battery {
            Some(b) => format!("{} {}%", d.name, b),
            None => d.name.clone(),
        })
        .collect();
    let text = if lines.is_empty() {
        "Periph Status\nNo devices connected".to_owned()
    } else {
        lines.join("\n")
    };
    if text.chars().count() <= TOOLTIP_MAX {
        text
    } else {
        text.chars().take(TOOLTIP_MAX - 1).chain(['…']).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BatterySource, DeviceKind};

    #[test]
    fn notifies_once_when_dropping_below_threshold() {
        assert_eq!(evaluate_low(false, Some(50), 20), (false, false));
        assert_eq!(evaluate_low(false, Some(20), 20), (true, true));
        // Still low → no spam.
        assert_eq!(evaluate_low(true, Some(15), 20), (false, true));
        assert_eq!(evaluate_low(true, Some(5), 20), (false, true));
    }

    #[test]
    fn hysteresis_prevents_flapping() {
        // Slightly above the threshold but below + HYSTERESIS → stays alerted.
        assert_eq!(evaluate_low(true, Some(22), 20), (false, true));
        // Drops again → no repeated notification.
        assert_eq!(evaluate_low(true, Some(19), 20), (false, true));
        // Charged → reset.
        assert_eq!(evaluate_low(true, Some(25), 20), (false, false));
        // Drops again later → notifies again.
        assert_eq!(evaluate_low(false, Some(18), 20), (true, true));
    }

    #[test]
    fn unknown_level_keeps_state() {
        assert_eq!(evaluate_low(true, None, 20), (false, true));
        assert_eq!(evaluate_low(false, None, 20), (false, false));
    }

    fn dev(name: &str, battery: Option<u8>, connected: bool) -> Device {
        Device {
            id: name.into(),
            name: name.into(),
            kind: DeviceKind::Other,
            battery,
            connected,
            source: BatterySource::Pnp,
        }
    }

    #[test]
    fn tooltip_lists_connected_devices() {
        let t = tooltip_text(&[
            dev("Mouse", Some(80), true),
            dev("Buds", None, true),
            dev("Old", Some(10), false),
        ]);
        assert_eq!(t, "Mouse 80%\nBuds");
        assert!(tooltip_text(&[]).contains("No devices"));
    }

    #[test]
    fn tooltip_is_truncated() {
        let many: Vec<_> = (0..20).map(|i| dev(&format!("Device number {i}"), Some(50), true)).collect();
        let t = tooltip_text(&many);
        assert_eq!(t.chars().count(), TOOLTIP_MAX);
        assert!(t.ends_with('…'));
    }
}
