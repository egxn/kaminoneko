//! Turns the screenshot into the 384 px wide image that is sent to the printer.
//!
//! Three stages, so the editor can cache each one:
//! 1. [`tone`]: rotation, invert and contrast on the full-size image ([`Settings`]).
//! 2. [`scale`]: resize to the width the image takes on the paper.
//! 3. [`compose`]: place it on the paper strip ([`Placement`]); whatever falls outside is cut.

use image::imageops::{self, FilterType};
use image::{GrayImage, Luma};

use crate::printer::PAPER_WIDTH;

/// Smallest image width and paper length, in dots.
pub const MIN_DOTS: u32 = 16;
/// Largest image width: 4× the paper, for zooming into a detail.
pub const MAX_WIDTH: u32 = PAPER_WIDTH * 4;
/// Largest blank margin above the image, in dots (≈ 5 cm).
pub const MAX_TOP: i32 = 400;

/// Tone settings, applied to the whole image.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    /// Rotation in clockwise quarter turns.
    pub rotation: u8,
    pub invert: bool,
    pub auto_contrast: bool,
    pub brightness: i32,
    pub contrast: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { rotation: 0, invert: false, auto_contrast: true, brightness: 0, contrast: 0 }
    }
}

/// Where the image sits on the paper, in printer dots.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    /// Width of the image on the paper. More than [`PAPER_WIDTH`] cuts the sides.
    pub width: u32,
    /// Top-left corner, relative to the top-left of the print area. Negative values cut.
    pub x: i32,
    pub y: i32,
    /// Paper length. `None` ends the paper where the image ends.
    pub length: Option<u32>,
}

impl Default for Placement {
    /// Full print width, at the top of the paper.
    fn default() -> Self {
        Self { width: PAPER_WIDTH, x: 0, y: 0, length: None }
    }
}

impl Placement {
    /// Image height on the paper for a source of `size` (width, height).
    pub fn height(&self, size: (u32, u32)) -> u32 {
        ((size.1 as f64 * self.width as f64 / size.0.max(1) as f64).round() as u32).max(1)
    }

    /// Paper length in dots.
    pub fn length(&self, size: (u32, u32)) -> u32 {
        self.length.unwrap_or_else(|| (self.y + self.height(size) as i32).max(0) as u32).max(MIN_DOTS)
    }

    /// Keeps the image at least partly on the paper and the sizes within limits.
    pub fn clamp(mut self, size: (u32, u32)) -> Self {
        self.width = self.width.clamp(MIN_DOTS, MAX_WIDTH);
        let (w, h) = (self.width as i32, self.height(size) as i32);
        let min = MIN_DOTS as i32;
        self.x = self.x.clamp(min - w, PAPER_WIDTH as i32 - min);
        self.y = self.y.clamp(min - h, MAX_TOP);
        self.length = self.length.map(|l| l.clamp(MIN_DOTS, 8000));
        self
    }

    /// Horizontal position that centers the image on the paper.
    pub fn centered_x(&self) -> i32 {
        (PAPER_WIDTH as i32 - self.width as i32) / 2
    }
}

/// Stage 1: rotation, invert and contrast at full resolution.
pub fn tone(original: &GrayImage, s: &Settings) -> GrayImage {
    let mut img = match s.rotation % 4 {
        1 => imageops::rotate90(original),
        2 => imageops::rotate180(original),
        3 => imageops::rotate270(original),
        _ => original.clone(),
    };
    if s.invert {
        imageops::invert(&mut img);
    }
    // Before downscaling: the Lanczos halo around text fools the histogram
    // and the white background would end up gray.
    if s.auto_contrast {
        auto_contrast(&mut img);
    }
    if s.brightness != 0 || s.contrast != 0 {
        let k = (100 + s.contrast) as f32 / 100.0;
        let table: Vec<u8> = (0..256)
            .map(|v| ((v as f32 - 128.0) * k + 128.0 + s.brightness as f32 * 1.28).round().clamp(0.0, 255.0) as u8)
            .collect();
        for p in img.pixels_mut() {
            p[0] = table[p[0] as usize];
        }
    }
    img
}

/// Stage 2: resize to `width` dots. `fast` trades quality for speed while the user drags.
pub fn scale(toned: &GrayImage, width: u32, fast: bool) -> GrayImage {
    let height = Placement { width, ..Default::default() }.height(toned.dimensions());
    let filter = if fast { FilterType::Triangle } else { FilterType::Lanczos3 };
    imageops::resize(toned, width, height, filter)
}

/// Stage 3: the paper strip, [`PAPER_WIDTH`] wide, with the scaled image placed on it.
pub fn compose(scaled: &GrayImage, p: &Placement) -> GrayImage {
    let length = p.length(scaled.dimensions());
    let mut sheet = GrayImage::from_pixel(PAPER_WIDTH, length, Luma([255]));
    imageops::overlay(&mut sheet, scaled, p.x as i64, p.y as i64);
    sheet
}

/// All three stages at full quality.
pub fn render(original: &GrayImage, s: &Settings, p: &Placement) -> GrayImage {
    let toned = tone(original, s);
    let p = p.clamp(toned.dimensions());
    compose(&scale(&toned, p.width, false), &p)
}

/// Stretches the histogram, discarding the darkest 1 % and the brightest 1 %.
fn auto_contrast(img: &mut GrayImage) {
    let mut hist = [0u64; 256];
    for p in img.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let cutoff = total / 100;
    let (mut acc, mut lo) = (0u64, 0usize);
    while lo < 255 && acc + hist[lo] <= cutoff {
        acc += hist[lo];
        lo += 1;
    }
    let (mut acc, mut hi) = (0u64, 255usize);
    while hi > 0 && acc + hist[hi] <= cutoff {
        acc += hist[hi];
        hi -= 1;
    }
    if hi <= lo + 8 {
        return; // nearly flat image: leave it alone
    }
    let table: Vec<u8> = (0..256)
        .map(|v| (((v as f32 - lo as f32) * 255.0 / (hi - lo) as f32).round().clamp(0.0, 255.0)) as u8)
        .collect();
    for p in img.pixels_mut() {
        p[0] = table[p[0] as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, v: u8) -> GrayImage {
        GrayImage::from_pixel(w, h, Luma([v]))
    }

    #[test]
    fn default_placement_fills_the_print_width() {
        let sheet = render(&flat(800, 400, 0), &Settings::default(), &Placement::default());
        assert_eq!(sheet.dimensions(), (PAPER_WIDTH, 192));
    }

    #[test]
    fn rotation_swaps_the_aspect() {
        let s = Settings { rotation: 1, ..Settings::default() };
        let sheet = render(&flat(800, 400, 0), &s, &Placement::default());
        assert_eq!(sheet.dimensions(), (PAPER_WIDTH, 768));
    }

    #[test]
    fn compose_places_and_cuts() {
        let p = Placement { width: 100, x: -20, y: 10, length: None };
        let sheet = compose(&flat(100, 50, 0), &p);
        assert_eq!(sheet.dimensions(), (PAPER_WIDTH, 60));
        assert_eq!(sheet.get_pixel(0, 5)[0], 255, "blank margin above the image");
        assert_eq!(sheet.get_pixel(0, 10)[0], 0, "left side cut, the rest starts at x = 0");
        assert_eq!(sheet.get_pixel(79, 59)[0], 0);
        assert_eq!(sheet.get_pixel(80, 10)[0], 255, "image ends at x = 80");
    }

    #[test]
    fn image_above_the_paper_shortens_it() {
        let p = Placement { width: 100, x: 0, y: -30, length: None };
        assert_eq!(compose(&flat(100, 50, 0), &p).height(), 20);
    }

    #[test]
    fn fixed_length_cuts_or_extends() {
        let image = flat(100, 50, 0);
        let cut = compose(&image, &Placement { width: 100, x: 0, y: 0, length: Some(30) });
        assert_eq!(cut.height(), 30);
        let long = compose(&image, &Placement { width: 100, x: 0, y: 0, length: Some(80) });
        assert_eq!((long.height(), long.get_pixel(0, 79)[0]), (80, 255));
    }

    #[test]
    fn clamp_keeps_part_of_the_image_on_the_paper() {
        let p = Placement { width: 99_999, x: 99_999, y: -99_999, length: Some(1) }.clamp((100, 100));
        assert_eq!(p.width, MAX_WIDTH);
        assert_eq!(p.x, (PAPER_WIDTH - MIN_DOTS) as i32);
        assert_eq!(p.y, MIN_DOTS as i32 - MAX_WIDTH as i32);
        assert_eq!(p.length, Some(MIN_DOTS));
    }

    #[test]
    fn auto_contrast_whitens_a_gray_background() {
        let mut img = flat(100, 100, 200);
        for x in 0..100 {
            for y in 40..60 {
                img.put_pixel(x, y, Luma([40]));
            }
        }
        let toned = tone(&img, &Settings::default());
        assert_eq!((toned.get_pixel(0, 0)[0], toned.get_pixel(0, 50)[0]), (255, 0));
    }
}
