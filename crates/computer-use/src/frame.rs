//! Screenshot geometry and the image pipeline.
//!
//! A [`Frame`] is the image the model last saw: `shot_w × shot_h` pixels
//! downscaled from a `dev_w × dev_h` capture. All model coordinates are in
//! shot space; [`Frame::to_device`] maps them back. A [`Zoom`] is a crop of
//! the same capture re-encoded at full budget; coordinates given in zoom
//! space are mapped through [`Zoom::to_frame`] first.

use std::io::Cursor;

use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage, imageops::FilterType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub shot_w: u32,
    pub shot_h: u32,
    pub dev_w: u32,
    pub dev_h: u32,
}

impl Frame {
    pub fn scale(&self) -> f64 {
        if self.shot_w == 0 {
            1.0
        } else {
            f64::from(self.dev_w) / f64::from(self.shot_w)
        }
    }

    /// Map shot-space coordinates to device pixels, rejecting points outside
    /// the frame (a small tolerance forgives off-by-one at the far edge).
    pub fn to_device(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        let (w, h) = (f64::from(self.shot_w), f64::from(self.shot_h));
        if !x.is_finite() || !y.is_finite() || x < -0.5 || y < -0.5 || x > w + 0.5 || y > h + 0.5 {
            return Err(format!(
                "point ({x}, {y}) is outside the current frame {}x{}; take a new computer_screenshot or use coordinates from the last one",
                self.shot_w, self.shot_h
            ));
        }
        let sx = f64::from(self.dev_w) / w;
        let sy = f64::from(self.dev_h) / h;
        let dx = (x * sx).clamp(0.0, f64::from(self.dev_w.saturating_sub(1)));
        let dy = (y * sy).clamp(0.0, f64::from(self.dev_h.saturating_sub(1)));
        Ok((dx, dy))
    }

    pub fn describe(&self) -> String {
        if self.shot_w == self.dev_w && self.shot_h == self.dev_h {
            format!(
                "frame: {}x{} (device {}x{}, scale 1)",
                self.shot_w, self.shot_h, self.dev_w, self.dev_h
            )
        } else {
            format!(
                "frame: {}x{} (device {}x{}, scale {:.3})",
                self.shot_w,
                self.shot_h,
                self.dev_w,
                self.dev_h,
                self.scale()
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zoom {
    /// Region in frame (shot) space.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Size of the zoomed image the model saw.
    pub out_w: u32,
    pub out_h: u32,
}

impl Zoom {
    /// Map zoom-image coordinates to frame coordinates.
    pub fn to_frame(&self, zx: f64, zy: f64) -> Result<(f64, f64), String> {
        let (w, h) = (f64::from(self.out_w), f64::from(self.out_h));
        if !zx.is_finite()
            || !zy.is_finite()
            || zx < -0.5
            || zy < -0.5
            || zx > w + 0.5
            || zy > h + 0.5
        {
            return Err(format!(
                "point ({zx}, {zy}) is outside the current zoom image {}x{}",
                self.out_w, self.out_h
            ));
        }
        let fx = f64::from(self.x) + zx * f64::from(self.w) / w;
        let fy = f64::from(self.y) + zy * f64::from(self.h) / h;
        Ok((fx, fy))
    }

    pub fn describe(&self) -> String {
        format!(
            "zoom: {}x{} image of frame region x={}..{} y={}..{} (pass frame=\"zoom\" to act on zoom coordinates, or convert: frame_x = {} + zoom_x * {:.3}, frame_y = {} + zoom_y * {:.3})",
            self.out_w,
            self.out_h,
            self.x,
            self.x + self.w,
            self.y,
            self.y + self.h,
            self.x,
            f64::from(self.w) / f64::from(self.out_w),
            self.y,
            f64::from(self.h) / f64::from(self.out_h)
        )
    }
}

pub fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    image::load_from_memory(bytes).map_err(|e| format!("failed to decode screenshot: {e}"))
}

pub fn encode_png(img: &RgbImage) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| format!("failed to encode png: {e}"))?;
    Ok(buf.into_inner())
}

pub(crate) fn fit(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= max_edge || longest == 0 {
        return (w, h);
    }
    let ratio = f64::from(max_edge) / f64::from(longest);
    (
        ((f64::from(w) * ratio).round() as u32).max(1),
        ((f64::from(h) * ratio).round() as u32).max(1),
    )
}

/// Downscale a capture to `max_edge`, optionally overlay a labeled grid, and
/// return PNG bytes plus the resulting frame geometry.
pub fn prepare(capture: &[u8], max_edge: u32, grid: bool) -> Result<(Vec<u8>, Frame), String> {
    let img = decode(capture)?;
    let (dev_w, dev_h) = img.dimensions();
    if dev_w == 0 || dev_h == 0 {
        return Err("screenshot is empty".to_string());
    }
    let (shot_w, shot_h) = fit(dev_w, dev_h, max_edge);
    let mut rgb = if (shot_w, shot_h) == (dev_w, dev_h) {
        img.to_rgb8()
    } else {
        img.resize_exact(shot_w, shot_h, FilterType::CatmullRom)
            .to_rgb8()
    };
    if grid {
        draw_grid(&mut rgb);
    }
    let png = encode_png(&rgb)?;
    Ok((
        png,
        Frame {
            shot_w,
            shot_h,
            dev_w,
            dev_h,
        },
    ))
}

/// Crop `region` (frame space) out of the capture and scale it to `max_edge`.
pub fn zoom(
    capture: &[u8],
    frame: &Frame,
    region: (u32, u32, u32, u32),
    max_edge: u32,
) -> Result<(Vec<u8>, Zoom), String> {
    let (x, y, w, h) = region;
    if w < 8 || h < 8 {
        return Err("zoom region must be at least 8x8 frame pixels".to_string());
    }
    if x.saturating_add(w) > frame.shot_w || y.saturating_add(h) > frame.shot_h {
        return Err(format!(
            "zoom region x={x} y={y} w={w} h={h} exceeds the frame {}x{}",
            frame.shot_w, frame.shot_h
        ));
    }
    let img = decode(capture)?;
    let sx = f64::from(frame.dev_w) / f64::from(frame.shot_w);
    let sy = f64::from(frame.dev_h) / f64::from(frame.shot_h);
    let cx = ((f64::from(x) * sx).floor() as u32).min(frame.dev_w - 1);
    let cy = ((f64::from(y) * sy).floor() as u32).min(frame.dev_h - 1);
    let cw = ((f64::from(w) * sx).ceil() as u32).clamp(1, frame.dev_w - cx);
    let ch = ((f64::from(h) * sy).ceil() as u32).clamp(1, frame.dev_h - cy);
    let cropped = img.crop_imm(cx, cy, cw, ch);
    // Scale so the longest edge hits the budget (upscaling small regions
    // helps the model read fine text).
    let longest = cw.max(ch);
    let ratio = f64::from(max_edge) / f64::from(longest);
    let out_w = ((f64::from(cw) * ratio).round() as u32).max(1);
    let out_h = ((f64::from(ch) * ratio).round() as u32).max(1);
    let filter = if ratio > 1.0 {
        FilterType::Lanczos3
    } else {
        FilterType::CatmullRom
    };
    let rgb = cropped.resize_exact(out_w, out_h, filter).to_rgb8();
    let png = encode_png(&rgb)?;
    Ok((
        png,
        Zoom {
            x,
            y,
            w,
            h,
            out_w,
            out_h,
        },
    ))
}

/// Pick a grid step that yields at most 10 lines across the wider side.
fn grid_step(width: u32) -> u32 {
    for step in [50u32, 100, 200, 250, 500] {
        if width / step <= 10 {
            return step;
        }
    }
    500
}

const GRID_COLOR: Rgb<u8> = Rgb([255, 0, 200]);
const LABEL_COLOR: Rgb<u8> = Rgb([255, 255, 0]);
const OUTLINE_COLOR: Rgb<u8> = Rgb([0, 0, 0]);

fn draw_grid(img: &mut RgbImage) {
    let (w, h) = img.dimensions();
    let step = grid_step(w.max(h));
    let mut x = step;
    while x < w {
        for yy in 0..h {
            // Dashed vertical line so content stays readable.
            if yy % 6 < 4 {
                img.put_pixel(x, yy, GRID_COLOR);
            }
        }
        draw_label(img, x + 2, 2, &x.to_string());
        x += step;
    }
    let mut y = step;
    while y < h {
        for xx in 0..w {
            if xx % 6 < 4 {
                img.put_pixel(xx, y, GRID_COLOR);
            }
        }
        draw_label(img, 2, y + 2, &y.to_string());
        y += step;
    }
}

/// 3x5 bitmap digits, drawn at 2x with a 1px outline.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

fn draw_label(img: &mut RgbImage, x0: u32, y0: u32, text: &str) {
    const SCALE: u32 = 2;
    let (w, h) = img.dimensions();
    let mut cursor = x0;
    for ch in text.chars() {
        let Some(digit) = ch.to_digit(10) else {
            continue;
        };
        let glyph = DIGITS[digit as usize];
        // Outline pass then fill pass.
        for pass in 0..2 {
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..3u32 {
                    if bits & (0b100 >> col) == 0 {
                        continue;
                    }
                    for dy in 0..SCALE {
                        for dx in 0..SCALE {
                            let px = cursor + col * SCALE + dx;
                            let py = y0 + row as u32 * SCALE + dy;
                            if pass == 0 {
                                for (ox, oy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                                    let (qx, qy) = (px as i64 + ox, py as i64 + oy);
                                    if qx >= 0 && qy >= 0 && (qx as u32) < w && (qy as u32) < h {
                                        img.put_pixel(qx as u32, qy as u32, OUTLINE_COLOR);
                                    }
                                }
                            } else if px < w && py < h {
                                img.put_pixel(px, py, LABEL_COLOR);
                            }
                        }
                    }
                }
            }
        }
        cursor += 4 * SCALE;
    }
}

#[cfg(test)]
pub(crate) fn synthetic_png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
    encode_png(&img).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_device_scales_and_bounds() {
        let frame = Frame {
            shot_w: 1024,
            shot_h: 640,
            dev_w: 2560,
            dev_h: 1600,
        };
        assert_eq!(frame.to_device(512.0, 320.0).unwrap(), (1280.0, 800.0));
        assert_eq!(frame.to_device(0.0, 0.0).unwrap(), (0.0, 0.0));
        let (x, y) = frame.to_device(1024.0, 640.0).unwrap();
        assert_eq!((x, y), (2559.0, 1599.0));
        assert!(frame.to_device(1100.0, 10.0).is_err());
        assert!(frame.to_device(-3.0, 10.0).is_err());
        assert!((frame.scale() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn zoom_maps_back_into_frame_space() {
        let zoom = Zoom {
            x: 100,
            y: 50,
            w: 200,
            h: 100,
            out_w: 1024,
            out_h: 512,
        };
        assert_eq!(zoom.to_frame(0.0, 0.0).unwrap(), (100.0, 50.0));
        assert_eq!(zoom.to_frame(1024.0, 512.0).unwrap(), (300.0, 150.0));
        assert_eq!(zoom.to_frame(512.0, 256.0).unwrap(), (200.0, 100.0));
        assert!(zoom.to_frame(2000.0, 0.0).is_err());
    }

    #[test]
    fn prepare_downscales_to_budget_and_keeps_aspect() {
        let capture = synthetic_png(2000, 1000);
        let (png, frame) = prepare(&capture, 1024, false).unwrap();
        assert_eq!(
            frame,
            Frame {
                shot_w: 1024,
                shot_h: 512,
                dev_w: 2000,
                dev_h: 1000
            }
        );
        let decoded = decode(&png).unwrap();
        assert_eq!(decoded.dimensions(), (1024, 512));
    }

    #[test]
    fn prepare_leaves_small_captures_alone_and_grid_keeps_size() {
        let capture = synthetic_png(640, 480);
        let (png, frame) = prepare(&capture, 1024, true).unwrap();
        assert_eq!(frame.shot_w, 640);
        assert_eq!(frame.shot_h, 480);
        let decoded = decode(&png).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (640, 480));
        // A grid line was painted at x=100.
        assert_eq!(*decoded.get_pixel(100, 0), GRID_COLOR);
    }

    #[test]
    fn zoom_crops_and_upscales_region() {
        let capture = synthetic_png(2000, 1000);
        let frame = Frame {
            shot_w: 1000,
            shot_h: 500,
            dev_w: 2000,
            dev_h: 1000,
        };
        let (png, zoom) = zoom(&capture, &frame, (100, 100, 200, 100), 800).unwrap();
        assert_eq!((zoom.out_w, zoom.out_h), (800, 400));
        assert_eq!(decode(&png).unwrap().dimensions(), (800, 400));
        assert!(zoom.describe().contains("x=100..300"));
        assert!(super::zoom(&capture, &frame, (900, 0, 200, 100), 800).is_err());
        assert!(super::zoom(&capture, &frame, (0, 0, 4, 4), 800).is_err());
    }

    #[test]
    fn grid_step_scales_with_width() {
        assert_eq!(grid_step(640), 100);
        assert_eq!(grid_step(1024), 100);
        assert_eq!(grid_step(2048), 200);
    }
}
