// user_prefs.json + the user avatar (data-formats.md §7).
//
// Clamp rules (applied both on load and in setters, as the old code did —
// EXCEPT permissionMode, which the old code only whitelisted in the setter;
// loading keeps the raw value):
//   - fontScale      → [0.85, 1.30], default 1.0
//   - preview sizes  → 0 (unset) or clamped to [320, 4096]
//   - userName       → trim + left(24); empty displays as "阿尔萨斯" (getter
//                      fallback, the stored value stays empty)
//   - permissionMode → setter whitelist default|plan|auto|yolo → "default"
//   - userAvatarPath → on load: must exist, else falls back to the fixed
//                      <root>/user_avatar.png if that exists
//   - panelLayout    → NEW field (old files lack it — must load fine):
//                      per-panel dock layout memory, see panels.md §1.2

use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::store::json::{left_chars, write_value_atomic, JsonError};
use crate::store::paths::Paths;

pub const DEFAULT_USER_NAME: &str = "阿尔萨斯";
pub const PERMISSION_MODES: [&str; 4] = ["default", "plan", "auto", "yolo"];

#[derive(Debug, thiserror::Error)]
pub enum PrefsError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
}

/// One panel's persisted layout entry (panels.md §1.2). All keys optional —
/// an entry may carry only `open`, only `height`, etc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelLayoutEntry {
    pub open: Option<bool>,
    pub height: Option<u32>,
    pub order: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// Field declaration order is alphabetical so serialized JSON keys come out
// sorted like QJsonObject wrote them (cosmetic diff-friendliness, §0).
#[derive(Debug, Clone, Serialize)]
pub struct UserPrefs {
    #[serde(rename = "fontScale")]
    font_scale: f64,
    #[serde(rename = "panelLayout")]
    panel_layout: Map<String, Value>,
    #[serde(rename = "permissionMode")]
    permission_mode: String,
    #[serde(rename = "previewHeight")]
    preview_height: i64,
    #[serde(rename = "previewWidth")]
    preview_width: i64,
    #[serde(rename = "userAvatarPath")]
    user_avatar_path: String,
    #[serde(rename = "userName")]
    user_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct PrefsFile {
    #[serde(rename = "permissionMode", default = "default_permission_mode")]
    permission_mode: String,
    #[serde(rename = "userAvatarPath")]
    user_avatar_path: String,
    #[serde(rename = "userName")]
    user_name: String,
    #[serde(rename = "previewWidth")]
    preview_width: i64,
    #[serde(rename = "previewHeight")]
    preview_height: i64,
    #[serde(rename = "fontScale", default = "default_font_scale")]
    font_scale: f64,
    #[serde(rename = "panelLayout")]
    panel_layout: Map<String, Value>,
}

impl Default for PrefsFile {
    fn default() -> Self {
        Self {
            permission_mode: default_permission_mode(),
            user_avatar_path: String::new(),
            user_name: String::new(),
            preview_width: 0,
            preview_height: 0,
            font_scale: default_font_scale(),
            panel_layout: Map::new(),
        }
    }
}

fn default_permission_mode() -> String {
    "default".to_string()
}

fn default_font_scale() -> f64 {
    1.0
}

impl Default for UserPrefs {
    fn default() -> Self {
        Self {
            permission_mode: default_permission_mode(),
            user_avatar_path: String::new(),
            user_name: String::new(),
            preview_width: 0,
            preview_height: 0,
            font_scale: 1.0,
            panel_layout: Map::new(),
        }
    }
}

/// Clamp to a sane window range; 0 clears back to the default.
fn clamp_preview_size(v: i64) -> i64 {
    if v <= 0 {
        0
    } else {
        v.clamp(320, 4096)
    }
}

/// Clamp to the advertised picker range (85% / 100% / 115% / 130%).
fn clamp_font_scale(v: f64) -> f64 {
    if !v.is_finite() {
        return 1.0;
    }
    v.clamp(0.85, 1.30)
}

impl UserPrefs {
    pub fn load(paths: &Paths) -> Self {
        let file: PrefsFile = fs::read(paths.user_prefs_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut prefs = Self {
            // NOTE: no whitelist here — the old load() took the raw string.
            permission_mode: file.permission_mode,
            user_avatar_path: String::new(),
            user_name: file.user_name,
            preview_width: clamp_preview_size(file.preview_width),
            preview_height: clamp_preview_size(file.preview_height),
            font_scale: clamp_font_scale(file.font_scale),
            panel_layout: file.panel_layout,
        };
        // Avatar existence check, with fallback to the fixed path.
        if !file.user_avatar_path.is_empty() && fs::metadata(&file.user_avatar_path).is_ok() {
            prefs.user_avatar_path = file.user_avatar_path;
        } else if paths.user_avatar_path().exists() {
            prefs.user_avatar_path = paths.user_avatar_path().to_string_lossy().into_owned();
        }
        prefs
    }

    pub fn save(&self, paths: &Paths) -> Result<(), PrefsError> {
        write_value_atomic(&paths.user_prefs_path(), self)?;
        Ok(())
    }

    // ---- permissionMode ----

    pub fn permission_mode(&self) -> &str {
        &self.permission_mode
    }

    pub fn set_permission_mode(&mut self, paths: &Paths, mode: &str) -> Result<(), PrefsError> {
        let m = mode.trim().to_lowercase();
        let m = if PERMISSION_MODES.contains(&m.as_str()) {
            m
        } else {
            default_permission_mode()
        };
        if self.permission_mode == m {
            return Ok(());
        }
        self.permission_mode = m;
        self.save(paths)
    }

    // ---- userName ----

    /// Display name with the "阿尔萨斯" fallback for the empty stored value.
    pub fn user_name(&self) -> String {
        if self.user_name.is_empty() {
            DEFAULT_USER_NAME.to_string()
        } else {
            self.user_name.clone()
        }
    }

    pub fn set_user_name(&mut self, paths: &Paths, name: &str) -> Result<(), PrefsError> {
        let n = left_chars(name.trim(), 24);
        if self.user_name == n {
            return Ok(());
        }
        self.user_name = n;
        self.save(paths)
    }

    // ---- preview size ----

    pub fn preview_width(&self) -> i64 {
        self.preview_width
    }

    pub fn preview_height(&self) -> i64 {
        self.preview_height
    }

    pub fn set_preview_width(&mut self, paths: &Paths, w: i64) -> Result<(), PrefsError> {
        let w = clamp_preview_size(w);
        if self.preview_width == w {
            return Ok(());
        }
        self.preview_width = w;
        self.save(paths)
    }

    pub fn set_preview_height(&mut self, paths: &Paths, h: i64) -> Result<(), PrefsError> {
        let h = clamp_preview_size(h);
        if self.preview_height == h {
            return Ok(());
        }
        self.preview_height = h;
        self.save(paths)
    }

    // ---- fontScale ----

    pub fn font_scale(&self) -> f64 {
        self.font_scale
    }

    pub fn set_font_scale(&mut self, paths: &Paths, s: f64) -> Result<(), PrefsError> {
        let s = clamp_font_scale(s);
        if (self.font_scale - s).abs() < f64::EPSILON {
            return Ok(());
        }
        self.font_scale = s;
        self.save(paths)
    }

    // ---- avatar (§7.2) ----

    pub fn user_avatar_path(&self) -> &str {
        &self.user_avatar_path
    }

    /// setUserAvatarFromFile: decode → center-crop square → 128×128 PNG →
    /// fixed path <root>/user_avatar.png; the path is then stored in prefs.
    /// Accepts a `file:` URL. False when the input is missing/undecodable.
    pub fn set_user_avatar_from_file(&mut self, paths: &Paths, local_path: &str) -> Result<bool, PrefsError> {
        let path = file_url_to_local(local_path);
        if path.is_empty() || !std::path::Path::new(&path).is_file() {
            return Ok(false);
        }
        let img = match image::open(&path) {
            Ok(img) => img,
            Err(_) => return Ok(false),
        };
        // Center-crop to square, then scale to 128×128.
        let (w, h) = (img.width(), img.height());
        let s = w.min(h);
        let x = (w - s) / 2;
        let y = (h - s) / 2;
        let cropped = img.crop_imm(x, y, s, s).resize_exact(
            128,
            128,
            image::imageops::FilterType::Lanczos3,
        );
        let dest = paths.user_avatar_path();
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        cropped.save(&dest)?;
        self.user_avatar_path = dest.to_string_lossy().into_owned();
        self.save(paths)?;
        Ok(true)
    }

    pub fn clear_user_avatar(&mut self, paths: &Paths) -> Result<(), PrefsError> {
        if self.user_avatar_path.is_empty() {
            return Ok(());
        }
        let _ = fs::remove_file(&self.user_avatar_path);
        self.user_avatar_path.clear();
        self.save(paths)
    }

    // ---- panelLayout (new field; absent in old files) ----

    pub fn panel_layout(&self) -> &Map<String, Value> {
        &self.panel_layout
    }

    pub fn panel_layout_for(&self, panel_id: &str) -> Option<PanelLayoutEntry> {
        self.panel_layout
            .get(panel_id)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Replace one panel's layout entry. The 300ms debounce is a caller
    /// (Tauri command layer) concern; the store writes immediately.
    pub fn set_panel_layout(
        &mut self,
        paths: &Paths,
        panel_id: &str,
        entry: &PanelLayoutEntry,
    ) -> Result<(), PrefsError> {
        let value = serde_json::to_value(entry)
            .map_err(|e| JsonError::Serde {
                path: paths.user_prefs_path(),
                source: e,
            })?;
        self.panel_layout.insert(panel_id.to_string(), value);
        self.save(paths)
    }
}

/// `file:` URL → local path (old code used QUrl(path).toLocalFile()).
/// Handles file:///C:/…, file://C:/… and plain paths passed through.
fn file_url_to_local(input: &str) -> String {
    if !input.starts_with("file:") {
        return input.to_string();
    }
    let mut rest = input.trim_start_matches("file:").trim_start_matches('/').to_string();
    // file://localhost/C:/… — strip a "localhost/" authority if present.
    if let Some(stripped) = rest.strip_prefix("localhost/") {
        rest = stripped.to_string();
    }
    rest
}
