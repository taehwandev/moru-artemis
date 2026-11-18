use std::collections::HashMap;
use std::env;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use image::ImageFormat;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const U2_DEVICE_PORT: u16 = 9008;

#[derive(Debug, Clone)]
pub struct AdbClient {
    adb_bin: String,
    host: Option<String>,
    port: Option<String>,
    hierarchy_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Default for AdbClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbClient {
    pub fn new() -> Self {
        let adb_bin = resolve_adb_binary();
        let host = env::var("ADB_HOST").ok().filter(|v| !v.trim().is_empty());
        let port = env::var("ADB_PORT")
            .ok()
            .filter(|v| !v.trim().is_empty() && v.chars().all(|c| c.is_ascii_digit()));
        Self {
            adb_bin,
            host,
            port,
            hierarchy_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn validate_serial(serial: &str) -> Result<String, String> {
        let trimmed = serial.trim();
        if trimmed.is_empty() {
            return Err("device_serial must be explicitly selected from devices".to_string());
        }
        Ok(trimmed.to_string())
    }

    fn daemon_addr(&self) -> (String, u16) {
        let host = match self.host.as_deref() {
            Some("localhost") | None => "127.0.0.1".to_string(),
            Some(other) => other.to_string(),
        };
        let port = self
            .port
            .as_deref()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5037);
        (host, port)
    }

    async fn device_lock(&self, serial: &str) -> Arc<Mutex<()>> {
        let mut map = self.hierarchy_locks.lock().await;
        map.entry(serial.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
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
            .map_err(|e| format!("failed to execute adb ({}): {e}", self.adb_bin))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let msg = if !stderr.is_empty() { stderr } else { stdout };
            return Err(if msg.is_empty() {
                format!("adb exited with status {}", output.status)
            } else {
                format!("{} (status {})", msg, output.status)
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
        let raw_bytes = self
            .run_raw(&["-s", &serial, "exec-out", "screencap", "-p"], 15)
            .await?;
        let png_bytes = strip_png_prefix(&raw_bytes)
            .ok_or_else(|| "Screenshot capture failed (no PNG signature found)".to_string())?;
        let img = image::load_from_memory(png_bytes)
            .map_err(|e| format!("Failed to decode screenshot PNG: {e}"))?;
        let rgb = img.to_rgb8();
        let mut buf = Cursor::new(Vec::new());
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80);
        encoder
            .encode_image(&image::DynamicImage::ImageRgb8(rgb))
            .map_err(|e| format!("Failed to encode screenshot JPEG: {e}"))?;
        Ok(buf.into_inner())
    }

    /// Attempt a fast (~20ms) non-colliding hierarchy read from an active on-device
    /// uiautomator2 server (`tcp:9008`) via the ADB host smart-socket tunnel.
    async fn try_u2_hierarchy(&self, serial: &str) -> Option<String> {
        let (host, port) = self.daemon_addr();
        for params in [json!([true, 50]), json!([true, Value::Null])] {
            let res = timeout(
                Duration::from_secs(4),
                u2_jsonrpc_dump(&host, port, serial, params),
            )
            .await
            .ok()
            .flatten();
            if let Some(xml) = res {
                if let Some(idx) = xml.find("<hierarchy") {
                    return Some(xml[idx..].to_string());
                }
            }
        }
        None
    }

    pub async fn dump_hierarchy(&self, serial: &str) -> Result<String, String> {
        let serial = Self::validate_serial(serial)?;
        let lock = self.device_lock(&serial).await;
        let _guard = lock.lock().await;

        // Tier 1: If a uiautomator2 daemon is already holding Android's singleton
        // UiAutomation connection on tcp:9008, query it directly (~20ms) instead
        // of colliding with exit code 137 (SIGKILL).
        if let Some(xml) = self.try_u2_hierarchy(&serial).await {
            return Ok(xml);
        }

        // Tier 2: Direct `exec-out uiautomator dump --compressed /dev/tty` with
        // bounded retry when overlapping UiAutomation registration returns 137.
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                sleep(Duration::from_millis(250 * attempt as u64)).await;
                if let Some(xml) = self.try_u2_hierarchy(&serial).await {
                    return Ok(xml);
                }
            }
            match self
                .run_raw(
                    &[
                        "-s",
                        &serial,
                        "exec-out",
                        "uiautomator",
                        "dump",
                        "--compressed",
                        "/dev/tty",
                    ],
                    15,
                )
                .await
            {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some(idx) = text.find("<hierarchy") {
                        return Ok(text[idx..].to_string());
                    }
                    last_err = text.trim().to_string();
                }
                Err(err) => {
                    last_err = err;
                    if !is_uiautomation_conflict(&last_err) {
                        break;
                    }
                }
            }
        }

        // Tier 3: Fallback to temp file on device and read back.
        match self
            .shell(
                &serial,
                "uiautomator dump --compressed /data/local/tmp/moru_uidump.xml",
                15,
            )
            .await
        {
            Ok(dump_out) => {
                if dump_out.contains("ERROR:") {
                    return Err(dump_out.trim().to_string());
                }
                let xml_bytes = self
                    .run_raw(
                        &["-s", &serial, "exec-out", "cat", "/data/local/tmp/moru_uidump.xml"],
                        10,
                    )
                    .await?;
                let text = String::from_utf8_lossy(&xml_bytes);
                if let Some(idx) = text.find("<hierarchy") {
                    return Ok(text[idx..].to_string());
                }
                Ok(text.into_owned())
            }
            Err(err) => {
                if is_uiautomation_conflict(&err) || is_uiautomation_conflict(&last_err) {
                    Err(format!(
                        "UI hierarchy dump killed (exit 137): another active UiAutomation service \
                         (e.g. Android Studio Layout Inspector or an unresponsive test daemon) holds \
                         Android's singleton UiAutomation lock ({err})"
                    ))
                } else {
                    Err(err)
                }
            }
        }
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

fn is_uiautomation_conflict(err: &str) -> bool {
    err.contains("137")
        || err.contains("Killed")
        || err.contains("already registered")
        || err.contains("signal: 9")
}

fn resolve_adb_binary() -> String {
    if let Ok(explicit) = env::var("ADB_PATH") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(path_var) = env::var_os("PATH") {
        for dir in env::split_paths(&path_var) {
            let cand = dir.join(adb_exe_name());
            if cand.is_file() {
                return cand.to_string_lossy().into_owned();
            }
        }
    }
    let mut sdk_candidates: Vec<PathBuf> = Vec::new();
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(val) = env::var_os(var) {
            sdk_candidates.push(PathBuf::from(val).join("platform-tools").join(adb_exe_name()));
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        sdk_candidates.push(
            home.join("Library")
                .join("Android")
                .join("sdk")
                .join("platform-tools")
                .join(adb_exe_name()),
        );
        sdk_candidates.push(
            home.join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join(adb_exe_name()),
        );
    }
    sdk_candidates.push(PathBuf::from("/opt/homebrew/bin").join(adb_exe_name()));
    sdk_candidates.push(PathBuf::from("/usr/local/bin").join(adb_exe_name()));

    for cand in sdk_candidates {
        if cand.is_file() {
            return cand.to_string_lossy().into_owned();
        }
    }
    "adb".to_string()
}

fn adb_exe_name() -> &'static str {
    if cfg!(windows) {
        "adb.exe"
    } else {
        "adb"
    }
}

/// Strip any multi-display warning text emitted ahead of the 8-byte PNG signature.
pub fn strip_png_prefix(bytes: &[u8]) -> Option<&[u8]> {
    bytes
        .windows(PNG_SIGNATURE.len())
        .position(|w| w == PNG_SIGNATURE)
        .map(|idx| &bytes[idx..])
}

async fn adb_send_packet(stream: &mut TcpStream, payload: &str) -> Result<(), ()> {
    let framed = format!("{:04x}{payload}", payload.len());
    stream.write_all(framed.as_bytes()).await.map_err(|_| ())?;
    let mut status = [0u8; 4];
    stream.read_exact(&mut status).await.map_err(|_| ())?;
    if &status == b"OKAY" {
        Ok(())
    } else {
        Err(())
    }
}

async fn u2_jsonrpc_dump(
    adb_host: &str,
    adb_port: u16,
    serial: &str,
    params: Value,
) -> Option<String> {
    let mut stream = timeout(
        Duration::from_millis(400),
        TcpStream::connect((adb_host, adb_port)),
    )
    .await
    .ok()?
    .ok()?;

    adb_send_packet(&mut stream, &format!("host:transport:{serial}"))
        .await
        .ok()?;
    adb_send_packet(&mut stream, &format!("tcp:{U2_DEVICE_PORT}"))
        .await
        .ok()?;

    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "dumpWindowHierarchy",
        "params": params,
    }))
    .ok()?;

    let req = format!(
        "POST /jsonrpc/0 HTTP/1.1\r\n\
         Host: 127.0.0.1:{U2_DEVICE_PORT}\r\n\
         User-Agent: moru-artemis\r\n\
         Accept-Encoding: \r\n\
         Content-Type: application/json\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\r\n\
         {body}",
        body.len()
    );

    stream.write_all(req.as_bytes()).await.ok()?;
    let mut raw_resp = Vec::new();
    stream.read_to_end(&mut raw_resp).await.ok()?;

    let http_body = extract_http_body(&raw_resp)?;
    let parsed: Value = serde_json::from_slice(&http_body).ok()?;
    parsed.get("result")?.as_str().map(str::to_string)
}

fn extract_http_body(raw: &[u8]) -> Option<Vec<u8>> {
    let split_pos = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let headers = String::from_utf8_lossy(&raw[..split_pos]).to_ascii_lowercase();
    if !headers.starts_with("http/1.1 200") && !headers.starts_with("http/1.0 200") {
        return None;
    }
    let body = &raw[split_pos + 4..];
    if headers.contains("transfer-encoding: chunked") {
        decode_chunked(body)
    } else {
        Some(body.to_vec())
    }
}

fn decode_chunked(mut input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let crlf = input.windows(2).position(|w| w == b"\r\n")?;
        let len_str = std::str::from_utf8(&input[..crlf])
            .ok()?
            .split(';')
            .next()?
            .trim();
        let len = usize::from_str_radix(len_str, 16).ok()?;
        input = &input[crlf + 2..];
        if len == 0 {
            break;
        }
        if input.len() < len + 2 {
            return None;
        }
        out.extend_from_slice(&input[..len]);
        input = &input[len + 2..];
    }
    Some(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_multidisplay_warning_ahead_of_png() {
        let mut raw = b"[Warning] Multiple displays were found!\n".to_vec();
        raw.extend_from_slice(PNG_SIGNATURE);
        raw.extend_from_slice(b"IHDR");
        let stripped = strip_png_prefix(&raw).unwrap();
        assert_eq!(&stripped[..8], PNG_SIGNATURE);
    }

    #[test]
    fn decodes_chunked_and_plain_http_bodies() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"result\":\"ok\"}";
        assert_eq!(extract_http_body(plain).unwrap(), b"{\"result\":\"ok\"}");

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"resul\r\n8\r\nt\":\"ok\"}\r\n0\r\n\r\n";
        assert_eq!(extract_http_body(chunked).unwrap(), b"{\"result\":\"ok\"}");
    }
}
