//! Persisted tray choices (size, track ribbons, display) in settings.json.
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub struct Settings {
    pub scale: f32,
    pub track: bool,
    /// Display index: 0 = primary, then the others left-to-right.
    pub display: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self { scale: 1.0, track: true, display: 0 }
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        let Ok(txt) = std::fs::read_to_string(crate::log::home().join("settings.json")) else {
            return s;
        };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { return s };
        if let Some(x) = v.get("scale").and_then(Value::as_f64) {
            if [1.0, 1.5, 2.0].contains(&x) {
                s.scale = x as f32;
            }
        }
        if let Some(x) = v.get("track").and_then(Value::as_bool) {
            s.track = x;
        }
        if let Some(x) = v.get("display").and_then(Value::as_u64) {
            s.display = x as usize;
        }
        s
    }

    pub fn save(&self) {
        let v = json!({ "scale": self.scale, "track": self.track, "display": self.display });
        let _ = std::fs::write(crate::log::home().join("settings.json"), v.to_string());
    }
}
