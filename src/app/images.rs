//! Loading, caching, and sizing of slide images.

use crate::markdown::{Block, Slide};
use image::DynamicImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a URL image fetch may take before giving up. Images are
/// fetched once, when the presentation starts.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// A loaded image ready to draw: its terminal protocol state and natural
/// size in terminal cells.
struct ImageEntry {
    protocol: StatefulProtocol,
    cols: u16,
    rows: u16,
}

/// A decoded image, or the reason it could not be loaded.
pub type Decoded = Result<DynamicImage, String>;

/// Decode every image in the deck: network fetches and file reads. Needs
/// no terminal, so the player runs it before entering the TUI (a slow
/// URL then delays startup, never a blank alternate screen).
pub fn decode_all(slides: &[Slide], base: &Path) -> HashMap<String, Decoded> {
    let mut decoded = HashMap::new();
    for slide in slides {
        for block in &slide.blocks {
            if let Block::Image { source, .. } = block
                && !decoded.contains_key(source)
            {
                decoded.insert(source.clone(), decode_logged(source, base));
            }
        }
    }
    decoded
}

/// [`read_image`] with the outcome traced: what decoded (dimensions,
/// duration) or why it failed.
fn decode_logged(source: &str, base: &Path) -> Decoded {
    let started = std::time::Instant::now();
    let decoded = read_image(source, base);
    match &decoded {
        Ok(img) => tracing::debug!(
            source,
            width = img.width(),
            height = img.height(),
            elapsed = ?started.elapsed(),
            "decoded image"
        ),
        Err(reason) => tracing::warn!(source, reason, "image failed to load"),
    }
    decoded
}

/// Caches slide images as terminal-drawable entries. `None` cache values
/// record sources that failed to load (so we do not retry every frame);
/// their reasons queue in `errors` for the player to surface. A `None`
/// picker means no graphics support was detected (or the terminal was
/// never probed, e.g. in tests).
pub struct Images {
    picker: Option<Picker>,
    /// Directory of the .keynot file; relative image paths resolve here.
    base: PathBuf,
    cache: HashMap<String, Option<ImageEntry>>,
    errors: Vec<String>,
}

impl Images {
    pub fn new(picker: Option<Picker>, base: PathBuf) -> Self {
        Images {
            picker,
            base,
            cache: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Turn pre-decoded images into drawable entries.
    pub fn adopt(&mut self, decoded: HashMap<String, Decoded>) {
        for (source, result) in decoded {
            self.insert(source, result);
        }
    }

    /// Ensure every image on the slide is loaded (or marked failed).
    pub fn preload(&mut self, slide: &Slide) {
        for block in &slide.blocks {
            if let Block::Image { source, .. } = block {
                self.load(source);
            }
        }
    }

    /// Load every image in the deck (used after reload; startup goes
    /// through [`decode_all`] + [`Images::adopt`] instead).
    pub fn preload_all(&mut self, slides: &[Slide]) {
        for slide in slides {
            self.preload(slide);
        }
    }

    /// Forget everything (used on reload, so edited images are re-read).
    pub fn clear(&mut self) {
        self.cache.clear();
        self.errors.clear();
    }

    /// Load-failure reasons collected since the last call, oldest first.
    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    fn load(&mut self, source: &str) {
        if self.cache.contains_key(source) {
            return;
        }
        // Without graphics support the image can never draw; skip the
        // (possibly network-bound) decode entirely.
        if self.picker.is_none() {
            self.cache.insert(source.to_string(), None);
            return;
        }
        let decoded = decode_logged(source, &self.base);
        self.insert(source.to_string(), decoded);
    }

    fn insert(&mut self, source: String, decoded: Decoded) {
        let entry = match (&self.picker, decoded) {
            (Some(picker), Ok(img)) => {
                let font = picker.font_size();
                let cols = img.width().div_ceil(u32::from(font.width.max(1))) as u16;
                let rows = img.height().div_ceil(u32::from(font.height.max(1))) as u16;
                Some(ImageEntry {
                    protocol: picker.new_resize_protocol(img),
                    cols: cols.max(1),
                    rows: rows.max(1),
                })
            }
            (_, Err(reason)) => {
                self.errors.push(format!("image {source}: {reason}"));
                None
            }
            (None, Ok(_)) => None,
        };
        self.cache.insert(source, entry);
    }

    /// The cell size an image should occupy within `max`, keeping aspect.
    pub fn fitted(&self, source: &str, max: (u16, u16)) -> Option<(u16, u16)> {
        match self.cache.get(source) {
            Some(Some(entry)) => Some(fit((entry.cols, entry.rows), max)),
            _ => None,
        }
    }

    pub fn protocol_mut(&mut self, source: &str) -> Option<&mut StatefulProtocol> {
        match self.cache.get_mut(source) {
            Some(Some(entry)) => Some(&mut entry.protocol),
            _ => None,
        }
    }
}

/// Read and decode an image from an `http(s)` URL or a filesystem path
/// (relative paths resolve against `base`). The error is a human-readable
/// reason; the renderer shows a placeholder for failed images.
fn read_image(source: &str, base: &Path) -> Decoded {
    if is_url(source) {
        fetch_image(source).map_err(|err| err.to_string())
    } else {
        let path = if Path::new(source).is_absolute() {
            PathBuf::from(source)
        } else {
            base.join(source)
        };
        image::open(path).map_err(|err| err.to_string())
    }
}

fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn fetch_image(url: &str) -> eyre::Result<DynamicImage> {
    tracing::debug!(url, "fetching image over http");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .new_agent();
    let bytes = agent.get(url).call()?.body_mut().read_to_vec()?;
    Ok(image::load_from_memory(&bytes)?)
}

/// Scale `natural` (cols, rows) down proportionally to fit within `max`.
/// Never scales up, and never returns a zero dimension.
fn fit(natural: (u16, u16), max: (u16, u16)) -> (u16, u16) {
    let (mut w, mut h) = (u32::from(natural.0.max(1)), u32::from(natural.1.max(1)));
    let (max_w, max_h) = (u32::from(max.0.max(1)), u32::from(max.1.max(1)));
    if w > max_w {
        h = (h * max_w / w).max(1);
        w = max_w;
    }
    if h > max_h {
        w = (w * max_h / h).max(1);
        h = max_h;
    }
    (w as u16, h as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_image_is_not_scaled_up() {
        assert_eq!(fit((10, 5), (80, 40)), (10, 5));
    }

    #[test]
    fn wide_image_clamps_to_width_keeping_aspect() {
        assert_eq!(fit((100, 50), (50, 100)), (50, 25));
    }

    #[test]
    fn tall_image_clamps_to_height_keeping_aspect() {
        assert_eq!(fit((50, 100), (100, 50)), (25, 50));
    }

    #[test]
    fn both_axes_clamp_when_needed() {
        // Width clamps 200 -> 40 (rows 100 -> 20), then height fits.
        assert_eq!(fit((200, 100), (40, 30)), (40, 20));
        // Height is still too big after the width clamp.
        assert_eq!(fit((100, 200), (40, 30)), (15, 30));
    }

    #[test]
    fn zero_dimensions_are_clamped_to_one() {
        assert_eq!(fit((0, 0), (10, 10)), (1, 1));
        assert_eq!(fit((10, 10), (0, 0)), (1, 1));
    }

    #[test]
    fn extreme_aspect_never_returns_zero() {
        let (w, h) = fit((1000, 1), (10, 10));
        assert!(w >= 1 && h >= 1);
        let (w, h) = fit((1, 1000), (10, 10));
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn url_sources_are_recognized() {
        assert!(is_url("https://example.com/a.png"));
        assert!(is_url("http://example.com/a.png"));
        assert!(!is_url("a.png"));
        assert!(!is_url("./https/a.png"));
        assert!(!is_url("/abs/path/a.png"));
        assert!(!is_url("ftp://example.com/a.png"));
    }

    #[test]
    fn images_without_picker_report_no_size() {
        let mut images = Images::new(None, PathBuf::from("."));
        images.load("whatever.png");
        assert_eq!(images.fitted("whatever.png", (80, 24)), None);
        assert!(images.protocol_mut("whatever.png").is_none());
    }

    #[test]
    fn missing_size_for_unloaded_source() {
        let images = Images::new(None, PathBuf::from("."));
        assert_eq!(images.fitted("never-loaded.png", (80, 24)), None);
    }
}
