use std::env;
use std::io::Cursor;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use image::ImageFormat;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct AdbClient {
    adb_bin: String,
    host: Option<String>,
    port: Option<String>,
}

impl Default for AdbClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbClient {
    pub fn new() -> Self {
        let adb_bin = env::var("ADB_PATH").unwrap_or_else(|_| "adb".to_string());
        let host = env::var("ADB_HOST").ok().filter(|v| !v.trim().is_empty());
        let port = env::var("ADB_PORT")
            .ok()
            .filter(|v| !v.trim().is_empty() && v.chars().all(|c| c.is_ascii_digit()));
        Self {
            adb_bin,
            host,
            port,
        }
    }

    pub fn validate_serial(serial: &str) -> Result<String, String> {
        let trimmed = serial.trim();
        if trimmed.is_empty() {
            return Err("device_serial must be explicitly selected from devices".to_string());
        }
        Ok(trimmed.to_string())
    }

    fn base_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.adb_bin);
        if let Some(host) = &self.host {
            cmd.arg("-H").arg(host);
        }
        if let Some(port) = &self.port {
            cmd.arg("-P").arg(port);
        }
        cmd.stdin(Stdio::null());
        cmd
    }

    async fn run_raw(&self, args: &[&str], wait_secs: u64) -> Result<Vec<u8>, String> {
        let mut cmd = self.base_cmd();
        cmd.args(args);
        let fut = cmd.output();
        let output = timeout(Duration::from_secs(wait_secs), fut)
            .await
            .map_err(|_| format!("adb command timed out after {wait_secs}s"))?
            .map_err(|e| format!("failed to execute adb: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let msg = if !stderr.is_empty() { stderr } else { stdout };
            return Err(if msg.is_empty() {
                format!("adb exited with status {}", output.status)
            } else {
                msg
            });
        }
        Ok(output.stdout)
    }

    pub async fn devices(&self) -> Result<Vec<String>, String> {
        let out = self.run_raw(&["devices"], 10).await?;
        let text = String::from_utf8_lossy(&out);
        let mut serials = Vec::new();
        for line in text.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            if let (Some(serial), Some(state)) = (parts.next(), parts.next()) {
                if state == "device" {
                    serials.push(serial.to_string());
                }
            }
        }
        Ok(serials)
    }

    pub async fn ensure_connected(&self, serial: &str) -> Result<String, String> {
        let serial = Self::validate_serial(serial)?;
        let connected = self.devices().await?;
        if connected.iter().any(|s| s == &serial) {
            Ok(serial)
        } else {
            Err("Selected device is not connected".to_string())
        }
    }

    pub async fn shell(&self, serial: &str, command: &str, wait_secs: u64) -> Result<String, String> {
        let serial = Self::validate_serial(serial)?;
        let out = self.run_raw(&["-s", &serial, "shell", command], wait_secs).await?;
        Ok(String::from_utf8_lossy(&out).to_string())
    }

    pub async fn pull(&self, serial: &str, remote: &str, local: &Path) -> Result<(), String> {
        let serial = Self::validate_serial(serial)?;
        let local_str = local
            .to_str()
            .ok_or_else(|| "invalid local path".to_string())?;
        self.run_raw(&["-s", &serial, "pull", remote, local_str], 30)
            .await?;
        Ok(())
    }

    pub async fn screenshot_jpeg(&self, serial: &str) -> Result<Vec<u8>, String> {
        let serial = Self::validate_serial(serial)?;
        let png_bytes = self
            .run_raw(&["-s", &serial, "exec-out", "screencap", "-p"], 15)
            .await?;
        if png_bytes.is_empty() {
            return Err("Screenshot capture failed".to_string());
        }
        let img = image::load_from_memory(&png_bytes)
            .map_err(|e| format!("Failed to decode screenshot PNG: {e}"))?;
        let rgb = img.to_rgb8();
        let mut buf = Cursor::new(Vec::new());
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80);
        encoder
            .encode_image(&image::DynamicImage::ImageRgb8(rgb))
            .map_err(|e| format!("Failed to encode screenshot JPEG: {e}"))?;
        Ok(buf.into_inner())
    }

    pub async fn dump_hierarchy(&self, serial: &str) -> Result<String, String> {
        let serial = Self::validate_serial(serial)?;
        // Fast path: dump directly to /dev/tty via exec-out
        if let Ok(bytes) = self
            .run_raw(&["-s", &serial, "exec-out", "uiautomator", "dump", "--compressed", "/dev/tty"], 15)
            .await
        {
            let text = String::from_utf8_lossy(&bytes);
            if let Some(idx) = text.find("<hierarchy") {
                return Ok(text[idx..].to_string());
            }
        }
        // Fallback path: dump to temp file on device and read back
        let dump_out = self
            .shell(&serial, "uiautomator dump --compressed /data/local/tmp/moru_uidump.xml", 15)
            .await?;
        if dump_out.contains("ERROR:") {
            return Err(dump_out.trim().to_string());
        }
        let xml_bytes = self
            .run_raw(&["-s", &serial, "exec-out", "cat", "/data/local/tmp/moru_uidump.xml"], 10)
            .await?;
        Ok(String::from_utf8_lossy(&xml_bytes).to_string())
    }

    pub async fn tap(&self, serial: &str, x: i32, y: i32) -> Result<(), String> {
        let serial = Self::validate_serial(serial)?;
        let cmd = format!("input tap {x} {y}");
        self.shell(&serial, &cmd, 10).await?;
        Ok(())
    }

    pub async fn swipe(
        &self,
        serial: &str,
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        duration_ms: i32,
    ) -> Result<(), String> {
        let serial = Self::validate_serial(serial)?;
        let cmd = format!("input swipe {start_x} {start_y} {end_x} {end_y} {duration_ms}");
        self.shell(&serial, &cmd, 15).await?;
        Ok(())
    }

    pub async fn press_key(&self, serial: &str, keycode: &str) -> Result<(), String> {
        let serial = Self::validate_serial(serial)?;
        let key = keycode.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err("Invalid keycode".to_string());
        }
        let cmd = format!("input keyevent {key}");
        self.shell(&serial, &cmd, 10).await?;
        Ok(())
    }

    pub async fn input_text(
        &self,
        serial: &str,
        text: &str,
        clear_existing: bool,
    ) -> Result<(), String> {
        let serial = Self::validate_serial(serial)?;
        if clear_existing {
            // Move to end, select all (CTRL+A via keycombination or long del), and clear
            let _ = self
                .shell(&serial, "input keycombination 113 29 && input keyevent 67", 10)
                .await;
        }
        if !text.is_empty() {
            let escaped = escape_adb_text(text);
            let cmd = format!("input text '{escaped}'");
            self.shell(&serial, &cmd, 15).await?;
        }
        Ok(())
    }

    pub async fn launch_app(&self, serial: &str, package_name: &str) -> Result<(), String> {
        let serial = Self::validate_serial(serial)?;
        let pkg = package_name.trim();
        if pkg.is_empty() || pkg.contains(char::is_whitespace) {
            return Err("package_name must be a valid Android package identifier".to_string());
        }
        let cmd = format!("monkey -p {pkg} -c android.intent.category.LAUNCHER 1");
        let out = self.shell(&serial, &cmd, 20).await?;
        if out.contains("No activities found to run") || out.contains("monkey aborted") {
            return Err(format!("App launch failed for {pkg}: {}", out.trim()));
        }
        Ok(())
    }
}

fn escape_adb_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for ch in input.chars() {
        match ch {
            ' ' => out.push_str("%s"),
            '\'' => out.push_str("'\\''"),
            '\\' | '"' | '&' | '|' | ';' | '<' | '>' | '(' | ')' | '$' | '`' | '*' | '?' | '#'
            | '~' | '=' | '%' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

pub fn encode_png(img: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| format!("Failed to encode PNG: {e}"))?;
    Ok(buf.into_inner())
}
