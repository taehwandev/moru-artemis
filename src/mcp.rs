use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::{json, Map, Value};
use tokio::time::sleep;

use crate::adb::AdbClient;
use crate::comparison::ComparisonStore;
use crate::elements::{
    fetch_elements, find_element_at_coords, hierarchy_json, input_element, scroll_to_element,
    tap_element, validate_selector, validate_wait_args, wait_for_element,
};
use crate::perf::{perf_frame_stats, record_frames};

#[derive(Clone)]
pub struct McpServer {
    adb: AdbClient,
    comparison: ComparisonStore,
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl McpServer {
    pub fn new() -> Self {
        Self {
            adb: AdbClient::new(),
            comparison: ComparisonStore::new(),
        }
    }

    pub async fn handle_message(&self, line: &str) -> Option<Value> {
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("Parse error: {e}") }
                }));
            }
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        // Notifications have no id and expect no response.
        let id = id?;

        let response = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "moru-artemis",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => self.call_tool(&params).await,
            other => Err((-32601, format!("Method not found: {other}"))),
        };

        Some(match response {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }),
        })
    }

    async fn call_tool(&self, params: &Value) -> Result<Value, (i32, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| (-32602, "Missing tool name".to_string()))?;
        let args = params
            .get("arguments")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        match self.dispatch_tool(name, &args).await {
            Ok(result) => Ok(result),
            Err(err) => Ok(json!({
                "content": [{ "type": "text", "text": err }],
                "isError": true,
            })),
        }
    }

    async fn dispatch_tool(&self, name: &str, args: &Map<String, Value>) -> Result<Value, String> {
        match name {
            "devices" => {
                let list = self.adb.devices().await?;
                Ok(text_result(&serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())))
            }
            "screenshot" => {
                let serial = req_str(args, "device_serial")?;
                let jpeg = self.adb.screenshot_jpeg(serial).await?;
                Ok(image_result(&jpeg, "image/jpeg"))
            }
            "hierarchy" => {
                let serial = req_str(args, "device_serial")?;
                let package = opt_str(args, "package")?;
                if let Some(pkg) = package {
                    if pkg.trim().is_empty() {
                        return Err("package must be a nonempty exact package name".to_string());
                    }
                }
                let max_nodes = opt_i64(args, "max_nodes")?;
                if let Some(n) = max_nodes {
                    if !(1..=64).contains(&n) {
                        return Err("max_nodes must be 1..64".to_string());
                    }
                }
                let labeled_only = opt_bool(args, "labeled_only")?.unwrap_or(false);
                let bounds_only = opt_bool(args, "bounds_only")?.unwrap_or(false);
                let elements = fetch_elements(&self.adb, serial).await?;
                let out = hierarchy_json(
                    &elements,
                    package,
                    max_nodes.map(|n| n as usize),
                    labeled_only,
                    bounds_only,
                );
                Ok(text_result(&out))
            }
            "tap" => {
                let serial = req_str(args, "device_serial")?;
                let x = req_i32(args, "x")?;
                let y = req_i32(args, "y")?;
                self.adb.tap(serial, x, y).await?;
                Ok(text_result("Success"))
            }
            "swipe" => {
                let serial = req_str(args, "device_serial")?;
                let start_x = req_i32(args, "start_x")?;
                let start_y = req_i32(args, "start_y")?;
                let end_x = req_i32(args, "end_x")?;
                let end_y = req_i32(args, "end_y")?;
                let duration_ms = opt_i32(args, "duration_ms")?.unwrap_or(400);
                if duration_ms <= 0 {
                    return Err("duration_ms must be positive".to_string());
                }
                self.adb
                    .swipe(serial, start_x, start_y, end_x, end_y, duration_ms)
                    .await?;
                Ok(text_result("Success"))
            }
            "input_text" => {
                let serial = req_str(args, "device_serial")?;
                let x = req_i32(args, "x")?;
                let y = req_i32(args, "y")?;
                let text = req_str(args, "text")?;
                let clear_existing = opt_bool(args, "clear_existing")?.unwrap_or(false);
                let elements = fetch_elements(&self.adb, serial).await?;
                let focused = find_element_at_coords(&elements, x, y)
                    .is_some_and(|el| el.is_focused());
                if !focused {
                    self.adb.tap(serial, x, y).await?;
                    sleep(Duration::from_secs(1)).await;
                }
                self.adb.input_text(serial, text, clear_existing).await?;
                Ok(text_result("Success"))
            }
            "press_key" => {
                let serial = req_str(args, "device_serial")?;
                let keycode = req_str(args, "keycode")?;
                self.adb.press_key(serial, keycode).await?;
                Ok(text_result("Success"))
            }
            "launch_app" => {
                let serial = req_str(args, "device_serial")?;
                let package_name = req_str(args, "package_name")?;
                self.adb.launch_app(serial, package_name).await?;
                Ok(text_result("Success"))
            }
            "tap_element" => {
                let serial = req_str(args, "device_serial")?;
                let selector = validate_selector(req_obj(args, "selector")?)?;
                let res = tap_element(&self.adb, serial, &selector).await?;
                Ok(json_result(&res))
            }
            "tap_element_and_wait" => {
                let serial = req_str(args, "device_serial")?;
                let selector = validate_selector(req_obj(args, "selector")?)?;
                let expected_obj = req_obj(args, "expected_selector")?;
                let expected_state = opt_str(args, "expected_state")?.unwrap_or("present");
                let timeout_ms = opt_i64(args, "timeout_ms")?.unwrap_or(5000);
                let poll_interval_ms = opt_i64(args, "poll_interval_ms")?.unwrap_or(250);
                let expected_sel =
                    validate_wait_args(expected_obj, expected_state, timeout_ms, poll_interval_ms)?;
                let action = tap_element(&self.adb, serial, &selector).await?;
                let verification = wait_for_element(
                    &self.adb,
                    serial,
                    &expected_sel,
                    expected_state,
                    timeout_ms,
                    poll_interval_ms,
                )
                .await?;
                Ok(json_result(&json!({
                    "action": action,
                    "verification": verification,
                })))
            }
            "input_element" => {
                let serial = req_str(args, "device_serial")?;
                let selector = validate_selector(req_obj(args, "selector")?)?;
                let text = req_str(args, "text")?;
                let clear_existing = opt_bool(args, "clear_existing")?.unwrap_or(false);
                let res = input_element(&self.adb, serial, &selector, text, clear_existing).await?;
                Ok(json_result(&res))
            }
            "scroll_to_element" => {
                let serial = req_str(args, "device_serial")?;
                let selector = validate_selector(req_obj(args, "selector")?)?;
                let start_x = req_i32(args, "start_x")?;
                let start_y = req_i32(args, "start_y")?;
                let end_x = req_i32(args, "end_x")?;
                let end_y = req_i32(args, "end_y")?;
                let max_swipes = opt_i32(args, "max_swipes")?.unwrap_or(5);
                let duration_ms = opt_i32(args, "duration_ms")?.unwrap_or(400);
                let settle_ms = opt_i32(args, "settle_ms")?.unwrap_or(250);
                let res = scroll_to_element(
                    &self.adb,
                    serial,
                    &selector,
                    start_x,
                    start_y,
                    end_x,
                    end_y,
                    max_swipes,
                    duration_ms,
                    settle_ms,
                )
                .await?;
                Ok(json_result(&res))
            }
            "wait_for_element" => {
                let serial = req_str(args, "device_serial")?;
                let sel_obj = req_obj(args, "selector")?;
                let state = opt_str(args, "state")?.unwrap_or("present");
                let timeout_ms = opt_i64(args, "timeout_ms")?.unwrap_or(5000);
                let poll_interval_ms = opt_i64(args, "poll_interval_ms")?.unwrap_or(250);
                let selector = validate_wait_args(sel_obj, state, timeout_ms, poll_interval_ms)?;
                let res = wait_for_element(
                    &self.adb,
                    serial,
                    &selector,
                    state,
                    timeout_ms,
                    poll_interval_ms,
                )
                .await?;
                Ok(json_result(&res))
            }
            "capture_baseline" => {
                let serial = req_str(args, "device_serial")?;
                self.comparison.capture_baseline(&self.adb, serial).await
            }
            "compare_current" => {
                let serial = req_str(args, "device_serial")?;
                self.comparison.compare_current(&self.adb, serial).await
            }
            "perf_frame_stats" => {
                let serial = req_str(args, "device_serial")?;
                let package_name = req_str(args, "package_name")?;
                let action = opt_str(args, "action")?.unwrap_or("read");
                let stages = opt_bool(args, "stages")?.unwrap_or(true);
                let res = perf_frame_stats(&self.adb, serial, package_name, action, stages).await?;
                Ok(json_result(&res))
            }
            "record_frames" => {
                let serial = req_str(args, "device_serial")?;
                let seconds = opt_i64(args, "seconds")?.unwrap_or(4);
                let fps = opt_i64(args, "fps")?.unwrap_or(4);
                let crop = opt_crop(args, "crop")?;
                let png = record_frames(&self.adb, serial, seconds, fps, crop).await?;
                Ok(image_result(&png, "image/png"))
            }
            other => Err(format!("Unknown tool: {other}")),
        }
    }
}

fn text_result(text: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    })
}

fn json_result(value: &Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(value).unwrap_or_else(|_| "{}".into()),
        }],
        "structuredContent": value,
        "isError": false,
    })
}

fn image_result(bytes: &[u8], mime_type: &str) -> Value {
    json!({
        "content": [{
            "type": "image",
            "data": BASE64.encode(bytes),
            "mimeType": mime_type,
        }],
        "isError": false,
    })
}

fn req_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn opt_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn req_i32(args: &Map<String, Value>, key: &str) -> Result<i32, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| format!("{key} must be an integer"))
}

fn opt_i32(args: &Map<String, Value>, key: &str) -> Result<Option<i32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("{key} must be an integer")),
    }
}

fn opt_i64(args: &Map<String, Value>, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be an integer")),
    }
}

fn opt_bool(args: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

fn req_obj<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a Map<String, Value>, String> {
    args.get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{key} must be an object"))
}

fn opt_crop(args: &Map<String, Value>, key: &str) -> Result<Option<[i64; 4]>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(arr)) if arr.len() == 4 => {
            let vals: Option<Vec<i64>> = arr.iter().map(Value::as_i64).collect();
            let vals = vals.ok_or_else(|| {
                "crop must be [left, top, right, bottom] pixel bounds".to_string()
            })?;
            Ok(Some([vals[0], vals[1], vals[2], vals[3]]))
        }
        Some(_) => Err("crop must be [left, top, right, bottom] pixel bounds".to_string()),
    }
}

fn tool_definitions() -> Value {
    let ro = json!({ "readOnlyHint": true });
    let serial_prop = json!({ "type": "string", "description": "Target ADB device serial" });
    let selector_prop = json!({
        "type": "object",
        "additionalProperties": { "type": "string" },
        "description": "Exact selector using text, content-desc, resource-id, class, or package"
    });

    json!([
        {
            "name": "devices",
            "description": "List connected ADB device serials. Select the intended device explicitly.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": ro
        },
        {
            "name": "screenshot",
            "description": "Return the current device screen as a JPEG MCP image.",
            "inputSchema": {
                "type": "object",
                "properties": { "device_serial": serial_prop },
                "required": ["device_serial"]
            },
            "annotations": ro
        },
        {
            "name": "hierarchy",
            "description": "Return UI elements, optionally filtered to a bounded JSON array.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "package": { "type": "string" },
                    "max_nodes": { "type": "integer", "minimum": 1, "maximum": 64 },
                    "labeled_only": { "type": "boolean", "default": false },
                    "bounds_only": { "type": "boolean", "default": false }
                },
                "required": ["device_serial"]
            },
            "annotations": ro
        },
        {
            "name": "wait_for_element",
            "description": "Poll locally for present/absent/enabled/disabled/focused/checked/selected/stable.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "selector": selector_prop,
                    "state": { "type": "string", "default": "present" },
                    "timeout_ms": { "type": "integer", "default": 5000 },
                    "poll_interval_ms": { "type": "integer", "default": 250 }
                },
                "required": ["device_serial", "selector"]
            },
            "annotations": ro
        },
        {
            "name": "capture_baseline",
            "description": "Capture the reference screen in memory before changing the app under test.",
            "inputSchema": {
                "type": "object",
                "properties": { "device_serial": serial_prop },
                "required": ["device_serial"]
            },
            "annotations": ro
        },
        {
            "name": "compare_current",
            "description": "Compare the current static screen with the captured same-device baseline.",
            "inputSchema": {
                "type": "object",
                "properties": { "device_serial": serial_prop },
                "required": ["device_serial"]
            },
            "annotations": ro
        },
        {
            "name": "tap",
            "description": "Tap screen pixel coordinates verified against a current observation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "x": { "type": "integer" },
                    "y": { "type": "integer" }
                },
                "required": ["device_serial", "x", "y"]
            }
        },
        {
            "name": "swipe",
            "description": "Swipe between screen pixel coordinates; duration is in milliseconds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "start_x": { "type": "integer" },
                    "start_y": { "type": "integer" },
                    "end_x": { "type": "integer" },
                    "end_y": { "type": "integer" },
                    "duration_ms": { "type": "integer", "default": 400 }
                },
                "required": ["device_serial", "start_x", "start_y", "end_x", "end_y"]
            }
        },
        {
            "name": "input_text",
            "description": "Focus an observed text field and type text; optionally replace its contents.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "text": { "type": "string" },
                    "clear_existing": { "type": "boolean", "default": false }
                },
                "required": ["device_serial", "x", "y", "text"]
            }
        },
        {
            "name": "press_key",
            "description": "Press an Android keycode, for example KEYCODE_BACK or KEYCODE_ENTER.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "keycode": { "type": "string" }
                },
                "required": ["device_serial", "keycode"]
            }
        },
        {
            "name": "launch_app",
            "description": "Launch the specified installed Android app on the selected device.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "package_name": { "type": "string" }
                },
                "required": ["device_serial", "package_name"]
            }
        },
        {
            "name": "tap_element",
            "description": "Tap one exact UI match without host coordinate calculation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "selector": selector_prop
                },
                "required": ["device_serial", "selector"]
            }
        },
        {
            "name": "tap_element_and_wait",
            "description": "Tap an exact match, then observe a bounded post-action condition.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "selector": selector_prop,
                    "expected_selector": selector_prop,
                    "expected_state": { "type": "string", "default": "present" },
                    "timeout_ms": { "type": "integer", "default": 5000 },
                    "poll_interval_ms": { "type": "integer", "default": 250 }
                },
                "required": ["device_serial", "selector", "expected_selector"]
            }
        },
        {
            "name": "input_element",
            "description": "Focus and type into one exact match; text is never echoed in the result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "selector": selector_prop,
                    "text": { "type": "string" },
                    "clear_existing": { "type": "boolean", "default": false }
                },
                "required": ["device_serial", "selector", "text"]
            }
        },
        {
            "name": "scroll_to_element",
            "description": "Find an exact element using bounded swipes and fresh local observations.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "selector": selector_prop,
                    "start_x": { "type": "integer" },
                    "start_y": { "type": "integer" },
                    "end_x": { "type": "integer" },
                    "end_y": { "type": "integer" },
                    "max_swipes": { "type": "integer", "default": 5 },
                    "duration_ms": { "type": "integer", "default": 400 },
                    "settle_ms": { "type": "integer", "default": 250 }
                },
                "required": ["device_serial", "selector", "start_x", "start_y", "end_x", "end_y"]
            }
        },
        {
            "name": "perf_frame_stats",
            "description": "Reset or summarize gfxinfo frames; never return the raw device dump.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "package_name": { "type": "string" },
                    "action": { "type": "string", "default": "read" },
                    "stages": { "type": "boolean", "default": true }
                },
                "required": ["device_serial", "package_name"]
            }
        },
        {
            "name": "record_frames",
            "description": "Record a short screen sequence and return one bounded PNG contact sheet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_serial": serial_prop,
                    "seconds": { "type": "integer", "default": 4 },
                    "fps": { "type": "integer", "default": 4 },
                    "crop": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "minItems": 4,
                        "maxItems": 4
                    }
                },
                "required": ["device_serial"]
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_and_list_tools() {
        let srv = McpServer::new();
        let init_resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await
            .unwrap();
        assert_eq!(init_resp["result"]["serverInfo"]["name"], "moru-artemis");

        let list_resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
            .await
            .unwrap();
        let tools = list_resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 17);
    }
}
