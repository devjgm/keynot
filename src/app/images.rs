//! Loading, caching, and sizing of slide images.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::DynamicImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::markdown::{Block, Slide};

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

/// Loads and caches slide images. `None` cache entries record files that
/// failed to load (so we do not retry every frame); a `None` picker means
/// no graphics support was detected (or the terminal was never probed,
/// e.g. in tests).
pub struct Images {
    picker: Option<Picker>,
    /// Directory of the .keynot file; relative image paths resolve here.
    base: PathBuf,
    cache: HashMap<String, Option<ImageEntry>>,
}

impl Images {
    pub fn new(picker: Option<Picker>, base: PathBuf) -> Self {
        Images {
            picker,
            base,
            cache: HashMap::new(),
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

    /// Load every image in the deck up front, so URL fetches happen when
    /// the presentation starts rather than stalling a mid-talk slide.
    pub fn preload_all(&mut self, slides: &[Slide]) {
        for slide in slides {
            self.preload(slide);
        }
    }

    /// Forget everything (used on reload, so edited images are re-read).
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    fn load(&mut self, source: &str) {
        if self.cache.contains_key(source) {
            return;
        }
        let Some(picker) = &self.picker else {
            self.cache.insert(source.to_string(), None);
            return;
        };
        let entry = read_image(source, &self.base).map(|img| {
            let font = picker.font_size();
            let cols = img.width().div_ceil(u32::from(font.width.max(1))) as u16;
            let rows = img.height().div_ceil(u32::from(font.height.max(1))) as u16;
            ImageEntry {
                protocol: picker.new_resize_protocol(img),
                cols: cols.max(1),
                rows: rows.max(1),
            }
        });
        self.cache.insert(source.to_string(), entry);
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
/// (relative paths resolve against `base`). `None` on any failure; the
/// renderer shows a placeholder instead.
fn read_image(source: &str, base: &Path) -> Option<DynamicImage> {
    if is_url(source) {
        fetch_image(source).ok()
    } else {
        let path = if Path::new(source).is_absolute() {
            PathBuf::from(source)
        } else {
            base.join(source)
        };
        image::open(path).ok()
    }
}

fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn fetch_image(url: &str) -> anyhow::Result<DynamicImage> {
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
