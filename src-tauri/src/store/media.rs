// media/ cache: directory layout, 14-day prune, clipboard image persistence
// (data-formats.md §9).
//
// Layout: media/<yyyy-MM-dd>/<sessionId|"no-session">/paste-yyyyMMdd-HHmmss-zzz.png|.jpg
// No session-deletion linkage (deleting a session does NOT delete its media —
// old-code "later pass" comment, kept deliberately).

use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;

use crate::store::paths::Paths;

/// Paste cache entries older than this many days are pruned at startup
/// (strictly greater: day 14 itself survives).
pub const DEFAULT_MAX_AGE_DAYS: i64 = 14;

/// Pasted images larger than this get downscaled / re-encoded.
const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_IMAGE_SIDE: u32 = 1920;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image encode error: {0}")]
    Image(#[from] image::ImageError),
}

/// media/<yyyy-MM-dd>/<sessionId>/ for TODAY (local date). Session ids are
/// app-generated and path-safe, but separators are stripped anyway since the
/// id flows in from the UI. Empty → "no-session".
pub fn media_dir_for(paths: &Paths, session_id: &str) -> PathBuf {
    media_dir_for_date(paths, session_id, chrono::Local::now().date_naive())
}

/// Testable variant with an explicit date.
pub fn media_dir_for_date(paths: &Paths, session_id: &str, date: NaiveDate) -> PathBuf {
    let leaf: String = session_id.chars().filter(|&c| c != '/' && c != '\\').collect();
    let leaf = if leaf.is_empty() { "no-session" } else { &leaf };
    paths
        .media_root()
        .join(date.format("%Y-%m-%d").to_string())
        .join(leaf)
}

/// Prune media/<yyyy-MM-dd>/ dirs whose date is strictly more than
/// `max_age_days` before `today`. Only directories whose name parses as a
/// yyyy-MM-dd date are touched; everything else (odd names, files) is left
/// alone. Returns the number of date dirs removed.
pub fn prune_media(paths: &Paths, max_age_days: i64, today: NaiveDate) -> Result<u32, MediaError> {
    let root = paths.media_root();
    if !root.is_dir() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(d) = parse_date_dir_name(name) else { continue };
        if (today - d).num_days() > max_age_days {
            fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Strict "yyyy-MM-dd" parse: exactly 10 chars, digits with '-' separators,
/// valid calendar date. Anything looser (e.g. "2026-7-5") is NOT a date dir
/// and is left alone by the prune.
fn parse_date_dir_name(name: &str) -> Option<NaiveDate> {
    let b = name.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return None;
    }
    NaiveDate::parse_from_str(name, "%Y-%m-%d").ok()
}

/// "清空缓存" UI entry: wipe the whole media/ tree. False when there was
/// nothing to remove.
pub fn clear_media_cache(paths: &Paths) -> bool {
    let root = paths.media_root();
    if !root.exists() {
        return false;
    }
    fs::remove_dir_all(&root).is_ok()
}

/// Clipboard image persistence with the legacy degradation chain
/// (data-formats.md §9.3). Input is the decoded clipboard bitmap (clipboard
/// access itself is platform glue, kept out of the store layer). Returns the
/// native-separator absolute path written into message `attachments`, or ""
/// on failure (old code returns an empty string, never throws).
///
/// Chain: PNG → if >2MB and any side >1920, downscale and retry PNG →
/// JPEG quality 90/75/60/45, first under 2MB wins → all fail: keep the LAST
/// (smallest) JPEG attempt.
pub fn save_clipboard_image(paths: &Paths, session_id: &str, img: image::RgbaImage) -> String {
    match save_clipboard_image_inner(paths, session_id, img) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("WarDex: save_clipboard_image failed: {e}");
            String::new()
        }
    }
}

fn save_clipboard_image_inner(
    paths: &Paths,
    session_id: &str,
    mut img: image::RgbaImage,
) -> Result<String, MediaError> {
    let dir = media_dir_for(paths, session_id);
    fs::create_dir_all(&dir)?;
    // Milliseconds in the name: pastes of one session share a dir, so two
    // pastes within the same second must not overwrite each other.
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
    let png_path = dir.join(format!("paste-{stamp}.png"));

    let mut png_bytes = encode_png(&img)?;
    fs::write(&png_path, &png_bytes)?;
    if png_bytes.len() as u64 <= MAX_IMAGE_BYTES {
        return Ok(native_separators(&png_path));
    }

    // Over the 2 MB cap: downscale to max side 1920 and retry as PNG first…
    if img.width().max(img.height()) > MAX_IMAGE_SIDE {
        img = image::imageops::resize(
            &img,
            MAX_IMAGE_SIDE,
            MAX_IMAGE_SIDE,
            image::imageops::FilterType::Lanczos3,
        );
        png_bytes = encode_png(&img)?;
        fs::write(&png_path, &png_bytes)?;
        if png_bytes.len() as u64 <= MAX_IMAGE_BYTES {
            return Ok(native_separators(&png_path));
        }
    }

    // …still too big: flatten alpha and step JPEG quality until under the cap.
    let rgb = image::DynamicImage::ImageRgba8(img).into_rgb8();
    let jpg_path = dir.join(format!("paste-{stamp}.jpg"));
    for quality in [90u8, 75, 60, 45] {
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        fs::write(&jpg_path, &buf)?;
        if buf.len() as u64 <= MAX_IMAGE_BYTES {
            let _ = fs::remove_file(&png_path);
            return Ok(native_separators(&jpg_path));
        }
    }
    // Gave up on shrinking: keep the last JPEG attempt (smallest of the set).
    let _ = fs::remove_file(&png_path);
    Ok(if jpg_path.exists() {
        native_separators(&jpg_path)
    } else {
        String::new()
    })
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, MediaError> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(buf)
}

/// QDir::toNativeSeparators equivalent for the path stored into attachments.
pub fn native_separators(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn parse_date_dir_name_strict() {
        assert!(parse_date_dir_name("2026-07-29").is_some());
        assert!(parse_date_dir_name("2026-7-29").is_none());
        assert!(parse_date_dir_name("2026-07-2x").is_none());
        assert!(parse_date_dir_name("2026-02-30").is_none());
        assert!(parse_date_dir_name("random").is_none());
        assert!(parse_date_dir_name("2026-07-29.bak").is_none());
    }

    #[test]
    fn prune_respects_strict_14_days() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        let media = paths.media_root();
        // day 15 before today → deleted; day 14 → kept; non-date dir → kept.
        for name in ["2026-07-01", "2026-07-02", "keepme", "2026-13-99"] {
            fs::create_dir_all(media.join(name)).unwrap();
        }
        let removed = prune_media(&paths, 14, date(2026, 7, 16)).unwrap();
        assert_eq!(removed, 1);
        assert!(!media.join("2026-07-01").exists());
        assert!(media.join("2026-07-02").exists()); // exactly 14 days: kept
        assert!(media.join("keepme").exists());
        assert!(media.join("2026-13-99").exists());
    }

    #[test]
    fn media_dir_strips_separators_and_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        let d = media_dir_for_date(&paths, "", date(2026, 7, 29));
        assert!(d.ends_with("2026-07-29\\no-session") || d.ends_with("2026-07-29/no-session"));
        let d = media_dir_for_date(&paths, "a/b\\c", date(2026, 7, 29));
        assert!(d.ends_with("abc"));
    }

    #[test]
    fn small_image_stays_png() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]));
        let out = save_clipboard_image(&paths, "s1", img);
        assert!(out.ends_with(".png"));
        assert!(std::path::Path::new(&out).exists());
    }
}
