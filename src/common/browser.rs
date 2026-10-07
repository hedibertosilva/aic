use std::process::{Command, Stdio};
/// The server is already listening before this is called. Failure leaves the URL usable.
pub fn open(url: &str) {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("No graphical session; open {url} manually (or use aic chat --no-open).");
        return;
    }
    std::thread::spawn({
        let url = url.to_owned();
        move || {
            let status = Command::new("xdg-open")
                .arg(&url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if !status.is_ok_and(|s| s.success()) {
                eprintln!("Could not open browser; open {url} manually.");
            }
        }
    });
}
