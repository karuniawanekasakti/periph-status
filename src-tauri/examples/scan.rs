//! Debug the battery provider without the UI:  cargo run --example scan
//! Compare the output with Settings › Bluetooth & devices.

fn main() -> anyhow::Result<()> {
    use periph_status_lib::provider;

    provider::init_thread();
    let mut p = provider::default_provider();
    let started = std::time::Instant::now();
    let devices = p.scan()?;

    println!("{} device(s) ({} ms)\n", devices.len(), started.elapsed().as_millis());
    println!("{:<32} {:<9} {:<10} {:>7}  {:<5}", "NAME", "KIND", "STATUS", "BATTERY", "SOURCE");
    for d in &devices {
        // Disconnected devices: only the last known value is available.
        let battery = match (d.battery, d.connected) {
            (Some(b), true) => format!("{b}%"),
            (Some(b), false) => format!("({b}%)"),
            (None, _) => "—".into(),
        };
        println!(
            "{:<32} {:<9} {:<10} {:>7}  {:<5}",
            truncate(&d.name, 32),
            format!("{:?}", d.kind).to_lowercase(),
            if d.connected { "connected" } else { "-" },
            battery,
            format!("{:?}", d.source).to_lowercase(),
        );
    }
    if devices.iter().any(|d| !d.connected && d.battery.is_some()) {
        println!("\n(…%) = last known value (stored by Windows / the receiver); the device is not connected");
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        s.chars().take(n - 1).chain(['…']).collect()
    }
}
