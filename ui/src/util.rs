use serde_json::Value;
use wasm_bindgen::JsValue;

pub fn window() -> web_sys::Window {
    web_sys::window().expect("browser window is available")
}

pub fn storage_get(key: &str) -> Option<String> {
    window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|storage| storage.get_item(key).ok().flatten())
}

pub fn storage_set(key: &str, value: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        let _ = storage.set_item(key, value);
    }
}

pub fn storage_remove(key: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        let _ = storage.remove_item(key);
    }
}

pub fn current_hash() -> String {
    window().location().hash().unwrap_or_default()
}

pub fn set_hash(hash: &str) {
    if current_hash() != hash {
        let _ = window().location().set_hash(hash);
    }
}

/// Parse the `#oidc=<outcome>&token=<session>` landing fragment. The fragment is
/// never sent to the server, so it is the safest place for the session token.
pub fn oidc_landing(hash: &str) -> Option<(String, Option<String>)> {
    let fragment = hash.strip_prefix('#')?;
    if !fragment.starts_with("oidc=") {
        return None;
    }
    let mut outcome = None;
    let mut token = None;
    for pair in fragment.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            match key {
                "oidc" => outcome = Some(value.to_string()),
                "token" => token = Some(value.to_string()),
                _ => {}
            }
        }
    }
    outcome.map(|outcome| (outcome, token))
}

pub fn reload() {
    let _ = window().location().reload();
}

pub fn relative(iso: &str) -> String {
    if iso.is_empty() {
        return "—".into();
    }
    let then = js_sys::Date::parse(iso);
    if then.is_nan() {
        return "—".into();
    }
    let seconds = ((js_sys::Date::now() - then) / 1000.0).max(0.0) as u64;
    if seconds < 5 {
        return "just now".into();
    }
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    full_date(iso)
}

pub fn full_date(iso: &str) -> String {
    let date = js_sys::Date::new(&JsValue::from_str(iso));
    if date.get_time().is_nan() {
        return "—".into();
    }
    date.to_locale_date_string("en-US", &JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_else(|| "—".into())
}

pub fn full_time(iso: &str) -> String {
    let date = js_sys::Date::new(&JsValue::from_str(iso));
    if date.get_time().is_nan() {
        return "—".into();
    }
    date.to_locale_string("en-US", &JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_else(|| "—".into())
}

pub fn short_time(iso: &str) -> String {
    let date = js_sys::Date::new(&JsValue::from_str(iso));
    if date.get_time().is_nan() {
        return "—".into();
    }
    date.to_locale_time_string("en-US")
        .as_string()
        .unwrap_or_else(|| "—".into())
}

pub fn num(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn pct(value: f64) -> String {
    format!("{}%", (value * 100.0).round() as i64)
}

pub fn cat_label(category: &str) -> String {
    match category {
        "raw_event" => "Raw event".into(),
        "issue" => "Issue".into(),
        "log" => "Log".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

pub fn level_badge_class(level: &str) -> &'static str {
    match level {
        "error" => "badge error",
        "fatal" => "badge fatal",
        "warning" => "badge warning",
        "info" => "badge info",
        _ => "badge debug",
    }
}

pub fn level_dot_class(level: &str) -> &'static str {
    match level {
        "error" => "level-dot error",
        "fatal" => "level-dot fatal",
        "warning" => "level-dot warning",
        "info" => "level-dot info",
        _ => "level-dot",
    }
}

pub fn state_badge_class(state: &str) -> &'static str {
    match state {
        "resolved" => "badge resolved",
        "ignored" => "badge ignored",
        _ => "badge open",
    }
}

pub fn role_badge_class(role: &str) -> &'static str {
    match role {
        "owner" => "badge owner",
        "admin" => "badge admin",
        "triager" => "badge triager",
        _ => "badge viewer",
    }
}

pub fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max).collect();
    out.push('…');
    out
}

pub fn encode(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_default()
}

pub fn iso_from_millis(millis: f64) -> String {
    js_sys::Date::new(&JsValue::from_f64(millis))
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
}

pub fn random_hex(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes * 2);
    for _ in 0..bytes * 2 {
        let digit = (js_sys::Math::random() * 16.0) as u32;
        out.push(std::char::from_digit(digit, 16).unwrap_or('0'));
    }
    out
}

pub fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub fn active_nav(view: &crate::state::ViewKind) -> &'static str {
    use crate::state::ViewKind;
    match view {
        ViewKind::Issues | ViewKind::Issue => "issues",
        ViewKind::Logs => "logs",
        ViewKind::Search => "search",
        ViewKind::Members => "members",
        ViewKind::Settings => "settings",
        ViewKind::Tokens => "tokens",
        ViewKind::Account => "account",
        ViewKind::Orgs => "orgs",
    }
}
