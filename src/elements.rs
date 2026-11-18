use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::time::{sleep, timeout};

use crate::adb::AdbClient;

const SELECTOR_FIELDS: &[&str] = &["text", "content-desc", "resource-id", "class", "package"];
const VALID_STATES: &[&str] = &[
    "present", "absent", "enabled", "disabled", "focused", "checked", "selected", "stable",
];
const VIEW_IDENTITY: &[&str] = &[
    "bounds",
    "class",
    "content-desc",
    "package",
    "resource-id",
    "text",
];
const VIEW_WHEN_TRUE: &[&str] = &[
    "checkable",
    "checked",
    "clickable",
    "focused",
    "long-clickable",
    "scrollable",
    "selected",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub fn parse(raw: &str) -> Option<Self> {
        let re = Regex::new(r"^\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]$").ok()?;
        let caps = re.captures(raw.trim())?;
        let left = caps.get(1)?.as_str().parse().ok()?;
        let top = caps.get(2)?.as_str().parse().ok()?;
        let right = caps.get(3)?.as_str().parse().ok()?;
        let bottom = caps.get(4)?.as_str().parse().ok()?;
        Some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    pub fn center(self) -> Result<(i32, i32), String> {
        if self.left < 0 || self.top < 0 || self.right <= self.left || self.bottom <= self.top {
            return Err("Element bounds are offscreen or empty".to_string());
        }
        Ok(((self.left + self.right) / 2, (self.top + self.bottom) / 2))
    }
}

#[derive(Debug, Clone, Default)]
pub struct UiElement {
    pub attrs: BTreeMap<String, String>,
    pub bools: BTreeMap<String, bool>,
    pub parsed_bounds: Option<Bounds>,
}

impl UiElement {
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        self.bools.get(key).copied()
    }

    pub fn is_focused(&self) -> bool {
        self.flag("focused") == Some(true)
    }

    pub fn center(&self) -> Result<(i32, i32), String> {
        if self.flag("enabled") != Some(true) {
            return Err("Element is disabled or enabled state is unknown; action refused".to_string());
        }
        let bounds = self
            .parsed_bounds
            .ok_or_else(|| "Element has no valid pixel bounds".to_string())?;
        bounds.center()
    }

    pub fn to_view(&self) -> Value {
        let mut map = Map::new();
        for &key in VIEW_IDENTITY {
            if let Some(val) = self.attrs.get(key) {
                if !val.is_empty() {
                    map.insert(key.to_string(), Value::String(val.clone()));
                }
            }
        }
        if let Some(enabled) = self.flag("enabled") {
            map.insert("enabled".to_string(), Value::Bool(enabled));
        }
        for &key in VIEW_WHEN_TRUE {
            if self.flag(key) == Some(true) {
                map.insert(key.to_string(), Value::Bool(true));
            }
        }
        Value::Object(map)
    }
}

pub fn parse_hierarchy_xml(xml: &str) -> Vec<UiElement> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut elements = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"node" {
                    let mut el = UiElement::default();
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = attr
                            .decode_and_unescape_value(reader.decoder())
                            .map(|v| v.into_owned())
                            .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).to_string());
                        match key.as_str() {
                            "resource-id" | "text" | "content-desc" | "class" | "package" => {
                                el.attrs.insert(key, val);
                            }
                            "bounds" => {
                                el.parsed_bounds = Bounds::parse(&val);
                                el.attrs.insert(key, val);
                            }
                            "checkable" | "checked" | "clickable" | "enabled" | "focusable"
                            | "focused" | "scrollable" | "long-clickable" | "password"
                            | "selected" => {
                                if val == "true" {
                                    el.bools.insert(key, true);
                                } else if val == "false" {
                                    el.bools.insert(key, false);
                                }
                            }
                            _ => {}
                        }
                    }
                    elements.push(el);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    elements
}

pub fn infer_screen_size(raw_elements: &[UiElement]) -> (i32, i32) {
    let default_w = 1080;
    let default_h = 2400;
    let mut best: Option<(i32, i32)> = None;
    let mut best_area: i64 = 0;
    for el in raw_elements {
        if let Some(b) = el.parsed_bounds {
            if b.left == 0 && b.top == 0 && b.right > 0 && b.bottom > 0 {
                let area = (b.right as i64) * (b.bottom as i64);
                if area > best_area {
                    best_area = area;
                    best = Some((b.right, b.bottom));
                }
            }
        }
    }
    if let Some((w, h)) = best {
        if best_area >= ((default_w as i64) * (default_h as i64)) / 2 {
            return (w, h);
        }
    }
    (default_w, default_h)
}

pub fn filter_elements(raw_elements: Vec<UiElement>, screen_w: i32, screen_h: i32) -> Vec<UiElement> {
    let min_size = 5;
    raw_elements
        .into_iter()
        .filter(|el| {
            let Some(b) = el.parsed_bounds else {
                return false;
            };
            if b.right <= 0 || b.bottom <= 0 || b.left >= screen_w || b.top >= screen_h {
                return false;
            }
            if b.width() < min_size || b.height() < min_size {
                return false;
            }
            let has_text = el.get_str("text").is_some_and(|s| !s.is_empty());
            let has_desc = el.get_str("content-desc").is_some_and(|s| !s.is_empty());
            let has_id = el.get_str("resource-id").is_some_and(|s| !s.is_empty());
            let interactive = el.flag("clickable") == Some(true)
                || el.flag("scrollable") == Some(true)
                || el.flag("checkable") == Some(true)
                || el.flag("long-clickable") == Some(true)
                || el.flag("focused") == Some(true);
            has_text || has_desc || has_id || interactive
        })
        .collect()
}

pub fn find_element_at_coords(elements: &[UiElement], x: i32, y: i32) -> Option<&UiElement> {
    elements
        .iter()
        .filter(|el| el.parsed_bounds.is_some_and(|b| b.contains(x, y)))
        .min_by_key(|el| {
            el.parsed_bounds
                .map(|b| (b.width() as i64) * (b.height() as i64))
                .unwrap_or(i64::MAX)
        })
}

pub fn validate_selector(selector: &Map<String, Value>) -> Result<Vec<(String, String)>, String> {
    if selector.is_empty() {
        return Err(
            "Use a nonempty selector with text, content-desc, resource-id, class or package"
                .to_string(),
        );
    }
    let mut pairs = Vec::with_capacity(selector.len());
    for (k, v) in selector {
        if !SELECTOR_FIELDS.contains(&k.as_str()) {
            return Err(
                "Use a nonempty selector with text, content-desc, resource-id, class or package"
                    .to_string(),
            );
        }
        let Some(s) = v.as_str() else {
            return Err("Selector values must be nonempty exact strings".to_string());
        };
        if s.trim().is_empty() {
            return Err("Selector values must be nonempty exact strings".to_string());
        }
        pairs.push((k.clone(), s.to_string()));
    }
    Ok(pairs)
}

pub fn match_element<'a>(
    elements: &'a [UiElement],
    selector: &[(String, String)],
) -> Result<Option<&'a UiElement>, String> {
    let matches: Vec<&UiElement> = elements
        .iter()
        .filter(|el| {
            selector
                .iter()
                .all(|(k, v)| el.get_str(k) == Some(v.as_str()))
        })
        .collect();
    if matches.len() > 1 {
        return Err(
            "Ambiguous selector; add resource-id, class or package to identify one element"
                .to_string(),
        );
    }
    Ok(matches.into_iter().next())
}

pub fn hierarchy_json(
    elements: &[UiElement],
    package: Option<&str>,
    max_nodes: Option<usize>,
    labeled_only: bool,
    bounds_only: bool,
) -> String {
    let mut views: Vec<Value> = elements
        .iter()
        .filter(|el| {
            if let Some(pkg) = package {
                if el.get_str("package") != Some(pkg) {
                    return false;
                }
            }
            if labeled_only {
                let has_text = el.get_str("text").is_some_and(|s| !s.is_empty());
                let has_desc = el.get_str("content-desc").is_some_and(|s| !s.is_empty());
                if !has_text && !has_desc {
                    return false;
                }
            }
            true
        })
        .map(|el| el.to_view())
        .collect();

    if bounds_only {
        views = views
            .into_iter()
            .filter_map(|v| {
                v.get("bounds")
                    .cloned()
                    .map(|b| json!({ "bounds": b }))
            })
            .collect();
    }

    if package.is_none() && max_nodes.is_none() && !labeled_only && !bounds_only {
        return serde_json::to_string(&views).unwrap_or_else(|_| "[]".to_string());
    }

    let limit = max_nodes.unwrap_or(64);
    let mut selected: Vec<Value> = Vec::new();
    let mut used_bytes = 2usize; // '[' and ']'
    for view in views.into_iter().take(limit) {
        let bounded = truncate_view_strings(view, 128);
        let item_bytes = serde_json::to_string(&bounded)
            .map(|s| s.len())
            .unwrap_or(0);
        let sep_bytes = if selected.is_empty() { 0 } else { 2 };
        if used_bytes + sep_bytes + item_bytes > 4096 {
            break;
        }
        selected.push(bounded);
        used_bytes += sep_bytes + item_bytes;
    }
    serde_json::to_string(&selected).unwrap_or_else(|_| "[]".to_string())
}

fn truncate_view_strings(value: Value, max_chars: usize) -> Value {
    match value {
        Value::Object(map) => {
            let out = map
                .into_iter()
                .map(|(k, v)| {
                    let v2 = match v {
                        Value::String(s) => Value::String(s.chars().take(max_chars).collect()),
                        other => other,
                    };
                    (k, v2)
                })
                .collect();
            Value::Object(out)
        }
        other => other,
    }
}

fn screen_signature(elements: &[UiElement]) -> Vec<(String, String, String, String)> {
    elements
        .iter()
        .map(|el| {
            (
                el.get_str("resource-id").unwrap_or("").to_string(),
                el.get_str("text").unwrap_or("").to_string(),
                el.get_str("content-desc").unwrap_or("").to_string(),
                el.get_str("bounds").unwrap_or("").to_string(),
            )
        })
        .collect()
}

pub async fn fetch_elements(adb: &AdbClient, serial: &str) -> Result<Vec<UiElement>, String> {
    let xml = adb.dump_hierarchy(serial).await?;
    let raw = parse_hierarchy_xml(&xml);
    let (w, h) = infer_screen_size(&raw);
    Ok(filter_elements(raw, w, h))
}

pub fn validate_wait_args(
    selector: &Map<String, Value>,
    state: &str,
    timeout_ms: i64,
    poll_interval_ms: i64,
) -> Result<Vec<(String, String)>, String> {
    let parsed = validate_selector(selector)?;
    if !VALID_STATES.contains(&state) {
        return Err("Unsupported UI state".to_string());
    }
    if !(1..=30000).contains(&timeout_ms) || !(100..=2000).contains(&poll_interval_ms) {
        return Err("timeout_ms must be 1..30000; poll_interval_ms must be 100..2000".to_string());
    }
    Ok(parsed)
}

pub async fn wait_for_element(
    adb: &AdbClient,
    serial: &str,
    selector: &[(String, String)],
    state: &str,
    timeout_ms: i64,
    poll_interval_ms: i64,
) -> Result<Value, String> {
    let start = Instant::now();
    let deadline = start + Duration::from_millis(timeout_ms as u64);
    let mut observations = 0usize;
    let mut last_element: Option<UiElement> = None;
    let mut last_bounds: Option<String> = None;

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        let elements = match timeout(remaining, fetch_elements(adb, serial)).await {
            Ok(Ok(els)) => els,
            Ok(Err(e)) => return Err(e),
            Err(_) => break,
        };
        observations += 1;
        let matched_el = match_element(&elements, selector)?.cloned();
        let mut matched = if state == "absent" {
            matched_el.is_none()
        } else {
            matched_el.is_some()
        };

        if matched && state == "stable" {
            let current_bounds = matched_el
                .as_ref()
                .and_then(|el| el.get_str("bounds"))
                .map(str::to_string);
            matched = current_bounds.is_some() && current_bounds == last_bounds;
            last_bounds = current_bounds;
        } else if matched && state != "present" && state != "absent" {
            let key = if state == "disabled" { "enabled" } else { state };
            let want = state != "disabled";
            matched = matched_el
                .as_ref()
                .and_then(|el| el.flag(key))
                == Some(want);
        } else {
            last_bounds = matched_el
                .as_ref()
                .and_then(|el| el.get_str("bounds"))
                .map(str::to_string);
        }

        last_element = matched_el;
        if matched && Instant::now() <= deadline {
            return Ok(json!({
                "matched": true,
                "status": "matched",
                "observations": observations,
                "elapsed_ms": start.elapsed().as_millis() as u64,
                "element": last_element.as_ref().map(UiElement::to_view),
            }));
        }

        let rem_sleep = deadline.saturating_duration_since(Instant::now());
        if rem_sleep.is_zero() {
            break;
        }
        let step = Duration::from_millis(poll_interval_ms as u64).min(rem_sleep);
        sleep(step).await;
    }

    Ok(json!({
        "matched": false,
        "status": "timeout",
        "observations": observations,
        "elapsed_ms": start.elapsed().as_millis() as u64,
        "element": last_element.as_ref().map(UiElement::to_view),
    }))
}

pub async fn tap_element(
    adb: &AdbClient,
    serial: &str,
    selector: &[(String, String)],
) -> Result<Value, String> {
    let elements = fetch_elements(adb, serial).await?;
    let element = match_element(&elements, selector)?
        .ok_or_else(|| "Element not found".to_string())?;
    let (x, y) = element.center()?;
    let view = element.to_view();
    adb.tap(serial, x, y).await?;
    Ok(json!({
        "status": "action_sent",
        "element": view,
        "point": [x, y],
    }))
}

pub async fn input_element(
    adb: &AdbClient,
    serial: &str,
    selector: &[(String, String)],
    text: &str,
    clear_existing: bool,
) -> Result<Value, String> {
    let elements = fetch_elements(adb, serial).await?;
    let element = match_element(&elements, selector)?
        .ok_or_else(|| "Element not found".to_string())?;
    let (x, y) = element.center()?;
    let focused = element.is_focused();
    if !focused {
        adb.tap(serial, x, y).await?;
        sleep(Duration::from_secs(1)).await;
    }
    adb.input_text(serial, text, clear_existing).await?;
    Ok(json!({
        "status": "action_sent",
        "point": [x, y],
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn scroll_to_element(
    adb: &AdbClient,
    serial: &str,
    selector: &[(String, String)],
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
    max_swipes: i32,
    duration_ms: i32,
    settle_ms: i32,
) -> Result<Value, String> {
    if start_x < 0 || start_y < 0 || end_x < 0 || end_y < 0 {
        return Err("Swipe coordinates must be nonnegative integer pixels".to_string());
    }
    if (start_x, start_y) == (end_x, end_y) {
        return Err("Swipe endpoints must differ".to_string());
    }
    if !(0..=10).contains(&max_swipes) {
        return Err("max_swipes must be 0..10".to_string());
    }
    if !(50..=2000).contains(&duration_ms) {
        return Err("duration_ms must be 50..2000".to_string());
    }
    if !(0..=2000).contains(&settle_ms) {
        return Err("settle_ms must be 0..2000".to_string());
    }

    let mut last_sig: Option<Vec<(String, String, String, String)>> = None;
    for swipes in 0..=max_swipes {
        let elements = fetch_elements(adb, serial).await?;
        let found = match_element(&elements, selector)?.is_some();
        let sig = screen_signature(&elements);
        let stagnant = settle_ms > 0 && !sig.is_empty() && last_sig.as_ref() == Some(&sig);
        if found || swipes == max_swipes || stagnant {
            let mut result = json!({
                "status": if found { "found" } else { "not_found" },
                "matched": found,
                "swipes": swipes,
                "observations": swipes + 1,
            });
            if stagnant && !found {
                result["scroll_end_reached"] = Value::Bool(true);
            }
            return Ok(result);
        }
        last_sig = Some(sig);
        adb.swipe(serial, start_x, start_y, end_x, end_y, duration_ms)
            .await?;
        sleep(Duration::from_millis(settle_ms as u64)).await;
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy rotation="0">
  <node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.example.app" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,0][1080,2400]">
    <node index="0" text="Sign In" resource-id="com.example.app:id/btn_login" class="android.widget.Button" package="com.example.app" content-desc="" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[100,400][500,520]" />
    <node index="1" text="Disabled" resource-id="com.example.app:id/btn_disabled" class="android.widget.Button" package="com.example.app" content-desc="" checkable="false" checked="false" clickable="true" enabled="false" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[100,600][500,720]" />
  </node>
</hierarchy>"#;

    #[test]
    fn parses_and_matches_exact_element() {
        let raw = parse_hierarchy_xml(SAMPLE_XML);
        assert_eq!(raw.len(), 3);
        let (w, h) = infer_screen_size(&raw);
        assert_eq!((w, h), (1080, 2400));
        let filtered = filter_elements(raw, w, h);
        assert_eq!(filtered.len(), 2);

        let sel = vec![("resource-id".to_string(), "com.example.app:id/btn_login".to_string())];
        let found = match_element(&filtered, &sel).unwrap().unwrap();
        assert_eq!(found.center().unwrap(), (300, 460));

        let disabled_sel = vec![("text".to_string(), "Disabled".to_string())];
        let disabled = match_element(&filtered, &disabled_sel).unwrap().unwrap();
        assert!(disabled.center().is_err());
    }

    #[test]
    fn hierarchy_json_respects_budget_and_filters() {
        let raw = parse_hierarchy_xml(SAMPLE_XML);
        let filtered = filter_elements(raw, 1080, 2400);
        let out = hierarchy_json(&filtered, Some("com.example.app"), Some(1), true, false);
        let parsed: Vec<Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["text"], "Sign In");
    }
}
