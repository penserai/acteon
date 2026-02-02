//! Simple image generation for Slack uploads.
//!
//! Provides procedural image generation that can be used with the
//! `upload_image` action type. Generated images are returned as PNG bytes
//! ready for upload.

use image::{ImageBuffer, Rgba, RgbaImage};
use std::io::Cursor;

use crate::error::SlackError;

/// Supported image generation styles.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSpec {
    /// A solid color rectangle.
    SolidColor {
        width: u32,
        height: u32,
        /// Color as hex string, e.g. "#FF5733" or "FF5733".
        color: String,
    },
    /// A horizontal gradient between two colors.
    Gradient {
        width: u32,
        height: u32,
        /// Start color as hex string.
        from_color: String,
        /// End color as hex string.
        to_color: String,
    },
    /// A checkerboard pattern.
    Checkerboard {
        width: u32,
        height: u32,
        /// Size of each square in pixels.
        square_size: u32,
        /// First color as hex string.
        color_a: String,
        /// Second color as hex string.
        color_b: String,
    },
    /// A simple bar chart rendered as an image.
    BarChart {
        width: u32,
        height: u32,
        /// Data values for each bar.
        values: Vec<f64>,
        /// Bar color as hex string.
        bar_color: String,
        /// Background color as hex string.
        #[serde(default = "default_bg_color")]
        background_color: String,
    },
}

fn default_bg_color() -> String {
    "#FFFFFF".to_owned()
}

/// Parse a hex color string into RGBA.
fn parse_hex_color(hex: &str) -> Result<Rgba<u8>, SlackError> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return Err(SlackError::InvalidPayload(format!(
            "invalid hex color: expected 6 hex digits, got '{hex}'"
        )));
    }
    let r = u8::from_str_radix(&hex[0..2], 16)
        .map_err(|_| SlackError::InvalidPayload(format!("invalid hex color: '{hex}'")))?;
    let g = u8::from_str_radix(&hex[2..4], 16)
        .map_err(|_| SlackError::InvalidPayload(format!("invalid hex color: '{hex}'")))?;
    let b = u8::from_str_radix(&hex[4..6], 16)
        .map_err(|_| SlackError::InvalidPayload(format!("invalid hex color: '{hex}'")))?;
    Ok(Rgba([r, g, b, 255]))
}

/// Linearly interpolate between two RGBA colors.
fn lerp_color(a: Rgba<u8>, b: Rgba<u8>, t: f64) -> Rgba<u8> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mix = |a: u8, b: u8| -> u8 {
        let v = f64::from(a) * (1.0 - t) + f64::from(b) * t;
        v.clamp(0.0, 255.0) as u8
    };
    Rgba([mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]), 255])
}

/// Validate image dimensions to prevent excessive memory allocation.
fn validate_dimensions(width: u32, height: u32) -> Result<(), SlackError> {
    const MAX_DIMENSION: u32 = 4096;
    const MAX_PIXELS: u64 = 16_777_216; // 4096 * 4096

    if width == 0 || height == 0 {
        return Err(SlackError::InvalidPayload(
            "image dimensions must be greater than zero".into(),
        ));
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(SlackError::InvalidPayload(format!(
            "image dimensions must not exceed {MAX_DIMENSION}x{MAX_DIMENSION}"
        )));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(SlackError::InvalidPayload(format!(
            "total pixel count must not exceed {MAX_PIXELS}"
        )));
    }
    Ok(())
}

/// Generate a PNG image from the given specification.
///
/// Returns the raw PNG bytes.
pub fn generate_image(spec: &ImageSpec) -> Result<Vec<u8>, SlackError> {
    let img = match spec {
        ImageSpec::SolidColor {
            width,
            height,
            color,
        } => {
            validate_dimensions(*width, *height)?;
            let rgba = parse_hex_color(color)?;
            ImageBuffer::from_fn(*width, *height, |_, _| rgba)
        }
        ImageSpec::Gradient {
            width,
            height,
            from_color,
            to_color,
        } => {
            validate_dimensions(*width, *height)?;
            let from = parse_hex_color(from_color)?;
            let to = parse_hex_color(to_color)?;
            ImageBuffer::from_fn(*width, *height, |x, _| {
                let t = if *width > 1 {
                    f64::from(x) / f64::from(*width - 1)
                } else {
                    0.0
                };
                lerp_color(from, to, t)
            })
        }
        ImageSpec::Checkerboard {
            width,
            height,
            square_size,
            color_a,
            color_b,
        } => {
            validate_dimensions(*width, *height)?;
            let sq = (*square_size).max(1);
            let a = parse_hex_color(color_a)?;
            let b = parse_hex_color(color_b)?;
            ImageBuffer::from_fn(*width, *height, |x, y| {
                if ((x / sq) + (y / sq)) % 2 == 0 {
                    a
                } else {
                    b
                }
            })
        }
        ImageSpec::BarChart {
            width,
            height,
            values,
            bar_color,
            background_color,
        } => {
            validate_dimensions(*width, *height)?;
            generate_bar_chart(*width, *height, values, bar_color, background_color)?
        }
    };

    encode_png(&img)
}

/// Generate a bar chart image.
fn generate_bar_chart(
    width: u32,
    height: u32,
    values: &[f64],
    bar_color: &str,
    background_color: &str,
) -> Result<RgbaImage, SlackError> {
    let bar_rgba = parse_hex_color(bar_color)?;
    let bg_rgba = parse_hex_color(background_color)?;

    let mut img = ImageBuffer::from_fn(width, height, |_, _| bg_rgba);

    if values.is_empty() {
        return Ok(img);
    }

    let max_val = values
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
        .max(0.001);

    let padding = 4u32;
    #[allow(clippy::cast_possible_truncation)]
    let n = values.len() as u32;
    let usable_width = width.saturating_sub(padding * 2);
    let bar_width = usable_width / n;
    if bar_width == 0 {
        return Ok(img);
    }
    let gap = (bar_width / 5).clamp(1, 4);
    let actual_bar = bar_width.saturating_sub(gap);

    let usable_height = height.saturating_sub(padding * 2);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    for (i, &val) in values.iter().enumerate() {
        let bar_height = ((val / max_val) * f64::from(usable_height)).round() as u32;
        let bar_height = bar_height.min(usable_height);
        let x_start = padding + (i as u32) * bar_width;
        let y_start = padding + usable_height - bar_height;

        for y in y_start..y_start + bar_height {
            for x in x_start..x_start + actual_bar {
                if x < width && y < height {
                    img.put_pixel(x, y, bar_rgba);
                }
            }
        }
    }

    Ok(img)
}

/// Encode an RGBA image as PNG bytes.
fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, SlackError> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| SlackError::InvalidPayload(format!("failed to encode PNG: {e}")))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_color_with_hash() {
        let c = parse_hex_color("#FF5733").unwrap();
        assert_eq!(c, Rgba([255, 87, 51, 255]));
    }

    #[test]
    fn parse_hex_color_without_hash() {
        let c = parse_hex_color("00FF00").unwrap();
        assert_eq!(c, Rgba([0, 255, 0, 255]));
    }

    #[test]
    fn parse_hex_color_invalid() {
        assert!(parse_hex_color("ZZZ").is_err());
        assert!(parse_hex_color("#12345").is_err());
    }

    #[test]
    fn generate_solid_color() {
        let spec = ImageSpec::SolidColor {
            width: 10,
            height: 10,
            color: "#FF0000".into(),
        };
        let png = generate_image(&spec).unwrap();
        assert!(!png.is_empty());
        // PNG magic bytes
        assert_eq!(&png[..4], &[137, 80, 78, 71]);
    }

    #[test]
    fn generate_gradient() {
        let spec = ImageSpec::Gradient {
            width: 100,
            height: 50,
            from_color: "#000000".into(),
            to_color: "#FFFFFF".into(),
        };
        let png = generate_image(&spec).unwrap();
        assert!(!png.is_empty());
    }

    #[test]
    fn generate_checkerboard() {
        let spec = ImageSpec::Checkerboard {
            width: 64,
            height: 64,
            square_size: 8,
            color_a: "#000000".into(),
            color_b: "#FFFFFF".into(),
        };
        let png = generate_image(&spec).unwrap();
        assert!(!png.is_empty());
    }

    #[test]
    fn generate_bar_chart() {
        let spec = ImageSpec::BarChart {
            width: 200,
            height: 100,
            values: vec![10.0, 25.0, 15.0, 30.0, 20.0],
            bar_color: "#3366CC".into(),
            background_color: "#FFFFFF".into(),
        };
        let png = generate_image(&spec).unwrap();
        assert!(!png.is_empty());
    }

    #[test]
    fn generate_bar_chart_empty_values() {
        let spec = ImageSpec::BarChart {
            width: 200,
            height: 100,
            values: vec![],
            bar_color: "#3366CC".into(),
            background_color: "#FFFFFF".into(),
        };
        let png = generate_image(&spec).unwrap();
        assert!(!png.is_empty());
    }

    #[test]
    fn reject_zero_dimensions() {
        let spec = ImageSpec::SolidColor {
            width: 0,
            height: 10,
            color: "#FF0000".into(),
        };
        assert!(generate_image(&spec).is_err());
    }

    #[test]
    fn reject_oversized_dimensions() {
        let spec = ImageSpec::SolidColor {
            width: 10000,
            height: 10000,
            color: "#FF0000".into(),
        };
        assert!(generate_image(&spec).is_err());
    }

    #[test]
    fn lerp_color_midpoint() {
        let a = Rgba([0, 0, 0, 255]);
        let b = Rgba([200, 100, 50, 255]);
        let mid = lerp_color(a, b, 0.5);
        assert_eq!(mid[0], 100);
        assert_eq!(mid[1], 50);
        assert_eq!(mid[2], 25);
    }
}
