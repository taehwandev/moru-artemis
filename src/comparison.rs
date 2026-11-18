use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use image::GenericImageView;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use crate::adb::AdbClient;
use crate::elements::fetch_elements;

const PIXEL_DELTA_THRESHOLD: i16 = 16;
const SEMANTIC_SAMPLE_LIMIT: usize = 20;

#[derive(Debug, Clone)]
pub struct BaselineState {
    pub device_serial: String,
    pub image_jpeg: Vec<u8>,
    pub elements: Vec<Value>,
    pub dimensions: (u32, u32),
}

#[derive(Debug, Default, Clone)]
pub struct ComparisonStore {
    baseline: Arc<Mutex<Option<BaselineState>>>,
}

impl ComparisonStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn capture_baseline(
        &self,
        adb: &AdbClient,
        device_serial: &str,
    ) -> Result<Value, String> {
        let serial = AdbClient::validate_serial(device_serial)?;
        let (image_jpeg, elements, dimensions) = capture(adb, &serial).await?;
        let count = elements.len();
        {
            let mut guard = self.baseline.lock().await;
            *guard = Some(BaselineState {
                device_serial: serial.clone(),
                image_jpeg: image_jpeg.clone(),
                elements,
                dimensions,
            });
        }
        let summary = json!({
            "status": "baseline_captured",
            "device_serial": serial,
            "dimensions": [dimensions.0, dimensions.1],
            "element_count": count,
            "storage": "memory_only",
        });
        Ok(observation_result(&summary, &image_jpeg))
    }

    pub async fn compare_current(
        &self,
        adb: &AdbClient,
        device_serial: &str,
    ) -> Result<Value, String> {
        let serial = AdbClient::validate_serial(device_serial)?;
        let baseline = {
            let guard = self.baseline.lock().await;
            guard
                .clone()
                .ok_or_else(|| "Capture a baseline before comparing the current screen".to_string())?
        };
        if serial != baseline.device_serial {
            return Err("Baseline belongs to a different device_serial".to_string());
        }
        let (image_jpeg, elements, dimensions) = capture(adb, &serial).await?;
        if dimensions != baseline.dimensions {
            return Err(
                "Screen dimensions changed; compare the same orientation and display state"
                    .to_string(),
            );
        }
        let p_delta = pixel_delta(&baseline.image_jpeg, &image_jpeg)?;
        let s_delta = semantic_delta(&baseline.elements, &elements);
        let summary = json!({
            "status": "comparison_observed",
            "device_serial": serial,
            "dimensions": [dimensions.0, dimensions.1],
            "pixel_delta": p_delta,
            "semantic_delta": s_delta,
            "verdict": "host_must_compare_against_explicit_acceptance_criteria",
        });
        Ok(observation_result(&summary, &image_jpeg))
    }
}

async fn capture(adb: &AdbClient, serial: &str) -> Result<(Vec<u8>, Vec<Value>, (u32, u32)), String> {
    let image_jpeg = adb.screenshot_jpeg(serial).await?;
    let raw = fetch_elements(adb, serial).await?;
    let elements: Vec<Value> = raw.iter().map(|el| el.to_view()).collect();
    let decoded = image::load_from_memory(&image_jpeg)
        .map_err(|e| format!("Failed to decode screenshot dimensions: {e}"))?;
    let dims = decoded.dimensions();
    Ok((image_jpeg, elements, dims))
}

pub fn pixel_delta(before_bytes: &[u8], current_bytes: &[u8]) -> Result<Value, String> {
    let first = image::load_from_memory(before_bytes)
        .map_err(|e| format!("Failed to decode baseline image: {e}"))?
        .to_rgb8();
    let second = image::load_from_memory(current_bytes)
        .map_err(|e| format!("Failed to decode current image: {e}"))?
        .to_rgb8();

    let (w, h) = first.dimensions();
    if second.dimensions() != (w, h) {
        return Err("Image dimensions differ".to_string());
    }
    let total_pixels = (w as usize) * (h as usize);
    if total_pixels == 0 {
        return Ok(json!({
            "changed_pixel_ratio": 0.0,
            "mean_absolute_channel_delta": 0.0,
            "threshold": PIXEL_DELTA_THRESHOLD,
            "changed_bounds": Value::Null,
        }));
    }

    let mut changed_pixels: usize = 0;
    let mut sum_delta: u64 = 0;
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..h {
        for x in 0..w {
            let p1 = first.get_pixel(x, y).0;
            let p2 = second.get_pixel(x, y).0;
            let dr = (p1[0] as i16 - p2[0] as i16).abs();
            let dg = (p1[1] as i16 - p2[1] as i16).abs();
            let db = (p1[2] as i16 - p2[2] as i16).abs();
            sum_delta += (dr + dg + db) as u64;
            if dr >= PIXEL_DELTA_THRESHOLD
                || dg >= PIXEL_DELTA_THRESHOLD
                || db >= PIXEL_DELTA_THRESHOLD
            {
                changed_pixels += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    let changed_ratio = ((changed_pixels as f64 / total_pixels as f64) * 1_000_000.0).round() / 1_000_000.0;
    let mean_delta =
        ((sum_delta as f64 / (total_pixels as f64 * 3.0)) * 1000.0).round() / 1000.0;
    let changed_bounds = if changed_pixels > 0 {
        json!([min_x, min_y, max_x + 1, max_y + 1])
    } else {
        Value::Null
    };

    Ok(json!({
        "changed_pixel_ratio": changed_ratio,
        "mean_absolute_channel_delta": mean_delta,
        "threshold": PIXEL_DELTA_THRESHOLD,
        "changed_bounds": changed_bounds,
    }))
}

fn identity_without_bounds(el: &Value) -> (String, String, String, String, String) {
    let s = |k: &str| {
        el.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    (
        s("resource-id"),
        s("text"),
        s("content-desc"),
        s("class"),
        s("package"),
    )
}

fn without_bounds(el: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(obj) = el.as_object() {
        for (k, v) in obj {
            if k != "bounds" {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

pub fn semantic_delta(before: &[Value], current: &[Value]) -> Value {
    let index = |items: &[Value]| -> BTreeMap<String, Value> {
        items
            .iter()
            .map(|v| {
                let key = serde_json::to_string(v).unwrap_or_default();
                (key, v.clone())
            })
            .collect()
    };

    let prev_map = index(before);
    let curr_map = index(current);
    let prev_keys: BTreeSet<String> = prev_map.keys().cloned().collect();
    let curr_keys: BTreeSet<String> = curr_map.keys().cloned().collect();

    let added_keys: Vec<String> = curr_keys.difference(&prev_keys).cloned().collect();
    let removed_keys: Vec<String> = prev_keys.difference(&curr_keys).cloned().collect();
    let added_items: Vec<Value> = added_keys.iter().map(|k| curr_map[k].clone()).collect();
    let removed_items: Vec<Value> = removed_keys.iter().map(|k| prev_map[k].clone()).collect();

    let count_idents = |items: &[Value]| {
        let mut counts = BTreeMap::new();
        for item in items {
            let ident = identity_without_bounds(item);
            if !ident.0.is_empty() || !ident.1.is_empty() || !ident.2.is_empty() {
                *counts.entry(ident).or_insert(0usize) += 1;
            }
        }
        counts
    };

    let before_counts = count_idents(before);
    let current_counts = count_idents(current);

    let mut removed_by_id: BTreeMap<_, Vec<Value>> = BTreeMap::new();
    for item in &removed_items {
        let ident = identity_without_bounds(item);
        if before_counts.get(&ident) == Some(&1) {
            removed_by_id.entry(ident).or_default().push(item.clone());
        }
    }
    let mut added_by_id: BTreeMap<_, Vec<Value>> = BTreeMap::new();
    for item in &added_items {
        let ident = identity_without_bounds(item);
        if current_counts.get(&ident) == Some(&1) {
            added_by_id.entry(ident).or_default().push(item.clone());
        }
    }

    let mut moved = Vec::new();
    for (ident, rem_group) in &removed_by_id {
        if let Some(add_group) = added_by_id.get(ident) {
            if rem_group.len() == 1 && add_group.len() == 1 {
                let prev_item = &rem_group[0];
                let curr_item = &add_group[0];
                let pb = prev_item.get("bounds").and_then(Value::as_str);
                let cb = curr_item.get("bounds").and_then(Value::as_str);
                if let (Some(pb), Some(cb)) = (pb, cb) {
                    if pb != cb && without_bounds(prev_item) == without_bounds(curr_item) {
                        moved.push(json!({
                            "element": curr_item,
                            "previous_bounds": pb,
                            "current_bounds": cb,
                        }));
                    }
                }
            }
        }
    }

    let truncated = added_keys.len() > SEMANTIC_SAMPLE_LIMIT
        || removed_keys.len() > SEMANTIC_SAMPLE_LIMIT
        || moved.len() > SEMANTIC_SAMPLE_LIMIT;

    json!({
        "baseline_element_count": before.len(),
        "current_element_count": current.len(),
        "added_count": added_keys.len(),
        "removed_count": removed_keys.len(),
        "moved_count": moved.len(),
        "added": added_items.into_iter().take(SEMANTIC_SAMPLE_LIMIT).collect::<Vec<_>>(),
        "removed": removed_items.into_iter().take(SEMANTIC_SAMPLE_LIMIT).collect::<Vec<_>>(),
        "moved": moved.into_iter().take(SEMANTIC_SAMPLE_LIMIT).collect::<Vec<_>>(),
        "samples_truncated": truncated,
    })
}

fn observation_result(summary: &Value, image_jpeg: &[u8]) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string(summary).unwrap_or_else(|_| "{}".to_string()),
            },
            {
                "type": "image",
                "data": BASE64.encode(image_jpeg),
                "mimeType": "image/jpeg",
            }
        ],
        "structuredContent": summary,
        "isError": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_delta_detects_moved_and_added() {
        let before = vec![json!({
            "resource-id": "btn",
            "text": "OK",
            "bounds": "[0,0][100,50]"
        })];
        let after = vec![
            json!({
                "resource-id": "btn",
                "text": "OK",
                "bounds": "[0,100][100,150]"
            }),
            json!({
                "resource-id": "badge",
                "text": "New",
                "bounds": "[10,10][30,30]"
            }),
        ];
        let delta = semantic_delta(&before, &after);
        assert_eq!(delta["moved_count"], 1);
        assert_eq!(delta["added_count"], 2);
        assert_eq!(delta["removed_count"], 1);
    }
}
