use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tracing::info;

pub struct BrowserLauncher;

impl BrowserLauncher {
    pub async fn ensure_chrome_running(
        port: u16,
        profile_dir: Option<PathBuf>,
    ) -> Result<String, String> {
        if Self::is_cdp_ready(port).await {
            info!("Chrome CDP is already listening on port {}", port);
            return Ok(format!("http://127.0.0.1:{}", port));
        }

        info!("Launching browser for authentication on port {}...", port);

        let data_dir = profile_dir.unwrap_or_else(dirs_profile_dir);

        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("Failed to create profile dir: {}", e))?;

        let port_flag = format!("--remote-debugging-port={}", port);
        let user_data_flag = format!("--user-data-dir={}", data_dir.display());

        let flatpak_status = Command::new("flatpak")
            .args(["info", "com.google.Chrome"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        let launched = if let Ok(status) = flatpak_status {
            if status.success() {
                info!("Launching Flatpak Google Chrome (com.google.Chrome)...");
                Command::new("flatpak")
                    .args([
                        "run",
                        "com.google.Chrome",
                        &port_flag,
                        &user_data_flag,
                        "https://gemini.google.com/app",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
            } else {
                Self::spawn_native_chrome(&port_flag, &user_data_flag)
            }
        } else {
            Self::spawn_native_chrome(&port_flag, &user_data_flag)
        };

        if let Err(e) = launched {
            return Err(format!("Failed to launch Chrome: {}", e));
        }

        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if Self::is_cdp_ready(port).await {
                info!(
                    "Chrome launched and CDP is ready at http://127.0.0.1:{}",
                    port
                );
                return Ok(format!("http://127.0.0.1:{}", port));
            }
        }

        Err(format!("Timed out waiting for Chrome CDP on port {}", port))
    }

    fn spawn_native_chrome(
        port_flag: &str,
        user_data_flag: &str,
    ) -> std::io::Result<std::process::Child> {
        let binaries = [
            "google-chrome-stable",
            "google-chrome",
            "chromium-browser",
            "chromium",
        ];
        for bin in binaries {
            if let Ok(child) = Command::new(bin)
                .args([port_flag, user_data_flag, "https://gemini.google.com/app"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                info!("Launched native browser binary: {}", bin);
                return Ok(child);
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No Chrome/Chromium binary found in PATH",
        ))
    }

    pub async fn is_cdp_ready(port: u16) -> bool {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap_or_default();

        let url = format!("http://127.0.0.1:{}/json/version", port);
        match client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }
}

fn dirs_profile_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config/dragoman/chrome_profile")
    } else {
        PathBuf::from(".dragoman_profile")
    }
}
