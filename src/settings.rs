//! Persisted tray choices (size, track ribbons, display, mode) in settings.json.
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub struct Settings {
    pub scale: f32,
    pub track: bool,
    /// Display index: 0 = primary, then the others left-to-right.
    pub display: usize,
    /// Mode: false = Overlayed (floats over windows), true = Exclusive (reserves
    /// a strip on the display's left edge; the usable area starts to its right).
    pub exclusive: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { scale: 1.0, track: true, display: 0, exclusive: false }
    }
}

impl Settings {
    /// Saved choices (kept across updates: the updater never touches the data
    /// dir). First run (no file): one display = Overlayed on it, 2+ = Exclusive
    /// on the last one; saved at once, so the preset does not follow later
    /// display changes.
    pub fn load(displays: usize) -> Self {
        let path = crate::log::data_dir().join("settings.json");
        if !path.exists() {
            let s = Self { display: displays.saturating_sub(1), exclusive: displays > 1, ..Self::default() };
            s.save();
            return s;
        }
        let mut s = Self::default();
        let Ok(txt) = std::fs::read_to_string(path) else {
            return s;
        };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { return s };
        if let Some(x) = v.get("scale").and_then(Value::as_f64) {
            if [0.75, 1.0, 1.25].contains(&x) {
                s.scale = x as f32;
            }
        }
        if let Some(x) = v.get("track").and_then(Value::as_bool) {
            s.track = x;
        }
        if let Some(x) = v.get("display").and_then(Value::as_u64) {
            s.display = x as usize;
        }
        if let Some(x) = v.get("exclusive").and_then(Value::as_bool) {
            s.exclusive = x;
        }
        s
    }

    pub fn save(&self) {
        let v = json!({ "scale": self.scale, "track": self.track, "display": self.display, "exclusive": self.exclusive });
        let _ = std::fs::write(crate::log::data_dir().join("settings.json"), v.to_string());
    }
}
