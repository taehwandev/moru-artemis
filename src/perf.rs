use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::process::Command;
use uuid::Uuid;

use crate::adb::{encode_png, AdbClient};

const STAGES: &[(&str, &str, &str)] = &[
    ("input_to_traversal", "HandleInputStart", "PerformTraversalsStart"),
    ("measure_layout", "PerformTraversalsStart", "DrawStart"),
    ("draw_recording", "DrawStart", "SyncStart"),
    ("render_thread", "SyncStart", "SwapBuffers"),
    ("swap", "SwapBuffers", "FrameCompleted"),
];

pub fn parse_gfxinfo(output: &str, include_stages: bool) -> Result<Map<String, Value>, String> {
    let extract_u64 = |pattern: &str| -> Option<u64> {
        let re = Regex::new(pattern).ok()?;
        for line in output.lines() {
            if let Some(caps) = re.captures(line) {
                if let Some(m) = caps.get(1) {
                    if let Ok(v) = m.as_str().parse::<u64>() {
                        return Some(v);
                    }
                }
            }
        }
        None
    };

    let total = extract_u64(r"^\s*Total frames rendered:\s*(\d+)");
    let janky = extract_u64(r"^\s*Janky frames:\s*(\d+)");
    let (Some(total), Some(janky)) = (total, janky) else {
        return Err("gfxinfo did not contain a frame summary for this package".to_string());
    };

    let jank_percent = if total > 0 {
        ((janky as f64 * 100.0 / total as f64) * 100.0).round() / 100.0
    } else {
        0.0
    };

    let mut result = Map::new();
    result.insert("total_frames".into(), json!(total));
    result.insert("janky_frames".into(), json!(janky));
    result.insert("jank_percent".into(), json!(jank_percent));
    result.insert(
        "missed_vsync".into(),
        json!(extract_u64(r"^\s*Number Missed Vsync:\s*(\d+)")),
    );
    result.insert(
        "slow_ui_thread".into(),
        json!(extract_u64(r"^\s*Number Slow UI thread:\s*(\d+)")),
    );
    result.insert(
        "slow_draw_commands".into(),
        json!(extract_u64(r"^\s*Number Slow issue draw commands:\s*(\d+)")),
    );

    let mut summary_pcts: Vec<(&str, Option<u64>)> = Vec::new();
    for &(label, pat) in &[
        ("p50", r"^\s*50th percentile:\s*(\d+)ms"),
        ("p90", r"^\s*90th percentile:\s*(\d+)ms"),
        ("p95", r"^\s*95th percentile:\s*(\d+)ms"),
        ("p99", r"^\s*99th percentile:\s*(\d+)ms"),
    ] {
        summary_pcts.push((label, extract_u64(pat)));
    }

    let has_any_pct = summary_pcts.iter().any(|(_, v)| v.is_some());
    let mut frame_time_map = Map::new();
    if has_any_pct {
        for (k, v) in &summary_pcts {
            frame_time_map.insert((*k).to_string(), json!(v));
        }
        result.insert("frame_time_ms".into(), Value::Object(frame_time_map.clone()));
    } else {
        result.insert("frame_time_ms".into(), Value::Null);
    }

    if include_stages {
        let mut stage_durations: HashMap<&str, Vec<f64>> =
            STAGES.iter().map(|(name, _, _)| (*name, Vec::new())).collect();
        let mut frame_durations: Vec<f64> = Vec::new();

        for row in profile_rows(output) {
            let start = row.get("IntendedVsync").copied().unwrap_or(0);
            let end = row.get("FrameCompleted").copied().unwrap_or(0);
            if start > 0 && end > start && (end - start) < 1_000_000_000 {
                frame_durations.push((end - start) as f64 / 1_000_000.0);
            }
            for &(name, first, last) in STAGES {
                let b = row.get(first).copied().unwrap_or(0);
                let f = row.get(last).copied().unwrap_or(0);
                if b > 0 && f > b && (f - b) < 1_000_000_000 {
                    if let Some(vec) = stage_durations.get_mut(name) {
                        vec.push((f - b) as f64 / 1_000_000.0);
                    }
                }
            }
        }

        result.insert("framestats_frames".into(), json!(frame_durations.len()));
        let mut stage_ms = Map::new();
        for &(name, _, _) in STAGES {
            let vals = stage_durations.get(name).map(Vec::as_slice).unwrap_or(&[]);
            stage_ms.insert(name.to_string(), percentiles(vals, &[50, 90, 99]));
        }
        result.insert("stage_ms".into(), Value::Object(stage_ms));

        if let Some(fb) = percentiles_map(&frame_durations, &[50, 90, 95, 99]) {
            let mut merged = Map::new();
            for (k, v) in summary_pcts {
                let val = match v {
                    Some(n) => json!(n),
                    None => fb.get(k).cloned().unwrap_or(Value::Null),
                };
                merged.insert(k.to_string(), val);
            }
            result.insert("frame_time_ms".into(), Value::Object(merged));
        }
    }

    Ok(result)
}

fn profile_rows(output: &str) -> Vec<HashMap<String, i64>> {
    let mut rows = Vec::new();
    for block in output.split("---PROFILEDATA---").skip(1).step_by(2) {
        let lines: Vec<String> = block
            .lines()
            .map(|l| l.trim().trim_matches(',').to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        let names: Vec<&str> = lines[0].split(',').collect();
        if !names.contains(&"IntendedVsync") || !names.contains(&"FrameCompleted") {
            continue;
        }
        for line in lines.iter().skip(1) {
            let vals: Vec<&str> = line.split(',').collect();
            if vals.len() != names.len() {
                continue;
            }
            let mut map = HashMap::new();
            let mut valid = true;
            for (k, v) in names.iter().zip(vals.iter()) {
                if let Ok(num) = v.parse::<i64>() {
                    map.insert((*k).to_string(), num);
                } else {
                    valid = false;
                    break;
                }
            }
            if valid {
                rows.push(map);
            }
        }
    }
    rows
}

fn percentiles_map(values: &[f64], levels: &[usize]) -> Option<Map<String, Value>> {
    if values.is_empty() {
        return None;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(|a, b| a.total_cmp(b));
    let mut out = Map::new();
    for &level in levels {
        let idx = (ordered.len() * level).div_ceil(100).saturating_sub(1);
        let rounded = (ordered[idx] * 100.0).round() / 100.0;
        out.insert(format!("p{level}"), json!(rounded));
    }
    Some(out)
}

fn percentiles(values: &[f64], levels: &[usize]) -> Value {
    match percentiles_map(values, levels) {
        Some(m) => Value::Object(m),
        None => Value::Null,
    }
}

pub async fn perf_frame_stats(
    adb: &AdbClient,
    device_serial: &str,
    package_name: &str,
    action: &str,
    stages: bool,
) -> Result<Value, String> {
    let pkg_re = Regex::new(r"^[A-Za-z_][A-Za-z0-9_.]{0,199}$").unwrap();
    if !pkg_re.is_match(package_name) {
        return Err("package_name must be an Android package identifier".to_string());
    }
    if action != "read" && action != "reset" {
        return Err("action must be read or reset".to_string());
    }
    let serial = adb.ensure_connected(device_serial).await?;
    let mut cmd = format!("dumpsys gfxinfo {package_name}");
    if action == "reset" {
        cmd.push_str(" reset");
    } else if stages {
        cmd.push_str(" framestats");
    }
    let output = adb.shell(&serial, &cmd, 20).await?;
    if output.contains("No process found") {
        return Err("No gfxinfo process found for this package".to_string());
    }
    if action == "reset" {
        return Ok(json!({
            "status": "reset",
            "device_serial": serial,
            "package_name": package_name,
        }));
    }
    let mut stats = parse_gfxinfo(&output, stages)?;
    stats.insert("device_serial".into(), json!(serial));
    stats.insert("package_name".into(), json!(package_name));
    Ok(Value::Object(stats))
}

pub async fn record_frames(
    adb: &AdbClient,
    device_serial: &str,
    seconds: i64,
    fps: i64,
    crop: Option<[i64; 4]>,
) -> Result<Vec<u8>, String> {
    if !(1..=10).contains(&seconds) || !(1..=4).contains(&fps) || seconds * fps > 16 {
        return Err("seconds must be 1..10, fps 1..4, and seconds*fps at most 16".to_string());
    }
    if let Some([l, t, r, b]) = crop {
        if l < 0 || t < 0 || r <= l || b <= t {
            return Err("crop must be [left, top, right, bottom] pixel bounds".to_string());
        }
    }
    let serial = adb.ensure_connected(device_serial).await?;
    let remote = format!("/sdcard/moru-artemis-{}.mp4", Uuid::new_v4().simple());
    let temp_dir = create_temp_dir()?;

    let res = record_and_build_sheet(adb, &serial, &remote, &temp_dir, seconds, fps, crop).await;
    let _ = adb.shell(&serial, &format!("rm -f {remote}"), 5).await;
    let _ = fs::remove_dir_all(&temp_dir);
    res
}

fn create_temp_dir() -> Result<PathBuf, String> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("moru-frames-{ts}-{}", Uuid::new_v4().simple()));
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create temp directory: {e}"))?;
    Ok(dir)
}

async fn record_and_build_sheet(
    adb: &AdbClient,
    serial: &str,
    remote: &str,
    temp_dir: &PathBuf,
    seconds: i64,
    fps: i64,
    crop: Option<[i64; 4]>,
) -> Result<Vec<u8>, String> {
    let local_mp4 = temp_dir.join("recording.mp4");
    let rec_cmd = format!("screenrecord --time-limit {seconds} --bit-rate 4000000 {remote}");
    let out = adb.shell(serial, &rec_cmd, (seconds as u64) + 15).await?;
    if out.contains("INVALID_LAYER_STACK") {
        return Err("Screen recording unavailable for the current display state".to_string());
    }
    if out.contains("ERROR:") || out.contains("error:") {
        return Err("Device screen recording failed".to_string());
    }
    adb.pull(serial, remote, &local_mp4).await?;
    let meta = fs::metadata(&local_mp4).map_err(|e| format!("Failed to stat recording: {e}"))?;
    if meta.len() == 0 {
        return Err("Device screen recording produced an empty video".to_string());
    }

    let pattern = temp_dir.join("frame-%03d.png");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            local_mp4.to_str().unwrap_or_default(),
            "-vf",
            &format!("fps={fps}"),
            "-frames:v",
            &(seconds * fps).to_string(),
            pattern.to_str().unwrap_or_default(),
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("Frame extraction failed (is ffmpeg installed?): {e}"))?;

    if !status.status.success() {
        return Err("Frame extraction failed".to_string());
    }

    let mut frame_paths: Vec<PathBuf> = fs::read_dir(temp_dir)
        .map_err(|e| format!("Failed to read extracted frames: {e}"))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("frame-") && n.ends_with(".png"))
        })
        .collect();
    frame_paths.sort();

    if frame_paths.is_empty() {
        return Err("Screen recording contained no frames".to_string());
    }

    let rows = frame_paths.len().div_ceil(4);
    let mut sheet = RgbImage::from_pixel(1024, (rows * 256) as u32, Rgb([0, 0, 0]));

    for (idx, path) in frame_paths.iter().enumerate() {
        let mut frame = image::open(path)
            .map_err(|e| format!("Failed to open frame: {e}"))?;
        if let Some([l, t, r, b]) = crop {
            let (fw, fh) = frame.dimensions();
            if r as u32 > fw || b as u32 > fh {
                return Err("crop is outside the recorded frame".to_string());
            }
            frame = frame.crop_imm(l as u32, t as u32, (r - l) as u32, (b - t) as u32);
        }
        let thumb = frame.resize(256, 232, FilterType::Triangle).to_rgb8();
        let x_base = ((idx % 4) * 256) as u32;
        let y_base = ((idx / 4) * 256) as u32;
        let x_off = x_base + (256 - thumb.width()) / 2;
        let y_off = y_base + 24;
        image::imageops::overlay(&mut sheet, &thumb, x_off as i64, y_off as i64);
    }

    encode_png(&DynamicImage::ImageRgb8(sheet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gfxinfo_summary() {
        let sample = "\
Total frames rendered: 120
Janky frames: 6 (5.00%)
50th percentile: 8ms
90th percentile: 14ms
95th percentile: 18ms
99th percentile: 25ms
Number Missed Vsync: 2
Number Slow UI thread: 1
Number Slow issue draw commands: 0
";
        let stats = parse_gfxinfo(sample, false).unwrap();
        assert_eq!(stats["total_frames"], 120);
        assert_eq!(stats["janky_frames"], 6);
        assert_eq!(stats["jank_percent"], 5.0);
        assert_eq!(stats["frame_time_ms"]["p90"], 14);
    }
}
