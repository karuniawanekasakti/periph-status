//! Autostart fix-ups on top of tauri-plugin-autostart.
//!
//! On Windows the plugin (via `auto-launch`) writes the `Run` registry value as
//! `<exe path> <args>` without quotes. With the installer's default location
//! (`%LOCALAPPDATA%\Periph Status`) that is an unquoted path with a space, which is fragile and a
//! known security weakness. The plugin also reports autostart as enabled whenever the value
//! exists, even if it points at an old executable. These helpers rewrite the value as
//! `"<current exe>" <args>`.

/// Command line stored in the `Run` key: quoted executable path followed by the arguments.
pub fn run_command(exe: &str, args: &[&str]) -> String {
    let mut cmd = format!("\"{exe}\"");
    for arg in args {
        cmd.push(' ');
        cmd.push_str(arg);
    }
    cmd
}

/// Point an existing autostart entry at the current executable, quoted.
/// Returns `true` if the registry value was changed; does nothing if autostart is not registered.
#[cfg(windows)]
pub fn repair_run_value(app_name: &str, args: &[&str]) -> anyhow::Result<bool> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    let exe = std::env::current_exe()?.display().to_string();
    let key = windows_registry::CURRENT_USER.create(RUN_KEY)?;
    let Ok(current) = key.get_string(app_name) else {
        return Ok(false);
    };
    let wanted = run_command(&exe, args);
    if current == wanted {
        return Ok(false);
    }
    key.set_string(app_name, &wanted)?;
    Ok(true)
}

#[cfg(not(windows))]
pub fn repair_run_value(_app_name: &str, _args: &[&str]) -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_path_with_spaces() {
        assert_eq!(
            run_command(r"C:\Users\me\AppData\Local\Periph Status\periph-status.exe", &["--autostart"]),
            r#""C:\Users\me\AppData\Local\Periph Status\periph-status.exe" --autostart"#
        );
    }

    #[test]
    fn no_args() {
        assert_eq!(run_command(r"C:\app.exe", &[]), r#""C:\app.exe""#);
    }
}
