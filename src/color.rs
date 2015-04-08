use super::config::{self, AdditiveColorConf};

use std::cmp::{max, min, partial_min};
use std::mem;

static RGB_SIZE: usize = 3; // RGB8 => 3 bytes, what LEDstream expects

type ColorTransformerConfig = config::Transform;
pub type HSVTransformer = config::HSV;

/// Just a simple modulo function, since % in rust is remainder
fn modulo(l: f32, r: f32) -> f32 {
	if l >= 0.0 {
		l % r
	} else {
		r + l % r
	}
}

/// Describes how to transform the red, green, and blue in an RGB pixel
#[derive(Clone)]
pub struct RgbTransformer {
	r: AdditiveColorConf,
	g: AdditiveColorConf,
	b: AdditiveColorConf
}
impl RgbTransformer {
	/// Construct new RGB color transformer from configs for each channel
	pub fn new(r: AdditiveColorConf, g: AdditiveColorConf, b: AdditiveColorConf)
		-> RgbTransformer
	{
		RgbTransformer{ r: r, g: g, b: b }
	}
}

/// Generic pixel. Transform the color without knowing the underlying format
pub trait Pixel {
	/// Convert the pixel to RGB8
	fn to_rgb(&self) -> RGB8;
	/// Convert the pixel to HSV
	fn to_hsv(&self) -> HSV;

	/// Transform the color of a pixel with RGB modifiers.
	fn rgb_transform(&self, rgb_transformer: &RgbTransformer) -> RGB8 {
		let rgb = self.to_rgb();
		let mut colors = [rgb.r, rgb.g, rgb.b];
		let transformers = [
			&rgb_transformer.r,
			&rgb_transformer.g,
			&rgb_transformer.b];

		for (color, transformer) in colors.iter_mut().zip(transformers.iter()) {
			let c = (*color as f32 / 255.0).powf(transformer.gamma)
				* transformer.whitelevel
				* (1.0 - transformer.blacklevel) + transformer.blacklevel;
			*color = (if c >= transformer.threshold { c } else { 0.0 }
				* 255.0) as u8;
		}
		RGB8{r: colors[0], g: colors[1], b: colors[2]}
	}

	/// Transform the color of a pixel with HSV modifiers.
	fn hsv_transform(&self, transformer: &HSVTransformer) -> HSV {
		let hsv = self.to_hsv();
		HSV{hue: hsv.hue,
			saturation: partial_min(1.0, hsv.saturation * transformer.saturationGain)
				.unwrap_or(1.0),
			value: partial_min(1.0, hsv.value * transformer.valueGain).unwrap_or(1.0)}
	}
}

/// Color format transmuted to from raw DXGCap color buffer.
///
/// Must contain only the fields [b, g, r, a], in that order,
/// or transmutation will behave unexpectedely
#[derive(Clone)]
pub struct BGRA8 {
	pub b: u8,
	pub g: u8,
	pub r: u8,
	pub a: u8
}

/// RGB pixel with 8 bits per color.
#[derive(Clone, Debug)]
pub struct RGB8 {
	pub r: u8,
	pub g: u8,
	pub b: u8
}
impl Pixel for RGB8 {
	fn to_rgb(&self) -> RGB8 {
		self.clone()
	}

	fn to_hsv(&self) -> HSV {
		let max = max(max(self.r, self.g), self.b);
		let min = min(min(self.r, self.g), self.b);
		let chroma = max - min;

		let hue = 1.0/6.0 * if chroma == 0 {
			0.0
		} else if max == self.r {
			modulo((self.g as f32 - self.b as f32) / chroma as f32, 6.0)
		} else if max == self.g {
			((self.b as f32 - self.r as f32) / chroma as f32) + 2.0
		} else {
			((self.r as f32 - self.g as f32) / chroma as f32) + 4.0
		};

		let value = max;

		let saturation = if value == 0 {
			0.0
		} else {
			chroma as f32 / value as f32
		};

		HSV{hue: hue, saturation: saturation, value: value as f32 / 255.0}
	}
}

/// A pixel in the [HSV](http://en.wikipedia.org/wiki/HSL_and_HSV) color format
#[derive(Clone, Debug)]
pub struct HSV {
	hue: f32,
	saturation: f32,
	value: f32
}
impl Pixel for HSV {
	fn to_rgb(&self) -> RGB8 {
		if self.saturation == 0.0 {
			let v = (self.value * 255.0) as u8;
			RGB8{r: v, g: v, b: v}
		} else {
			let sector_f = self.hue * 6.0;
			let sector = sector_f as u8;
			let factorial_part = sector_f - sector as f32;
			let val_255 = self.value * 255.0;
			let v_8bit = val_255 as u8;

			let p = (val_255 * (1.0 - self.saturation)) as u8;
			let q = (val_255 * (1.0 - self.saturation * factorial_part)) as u8;
			let t = (val_255 * (1.0 - self.saturation * (1.0 - factorial_part))) as u8;
			
			let (r, g, b) = match sector {
				0 => (v_8bit, t, p),
				1 => (q, v_8bit, p),
				2 => (p, v_8bit, t),
				3 => (p, q, v_8bit),
				4 => (t, p, v_8bit),
				_ => (v_8bit, p, q),
			};
			RGB8{r: r, g: g, b: b}
		}
	}

	fn to_hsv(&self) -> HSV {
		self.clone()
	}
}

/// Convert RGB8 pixels to same pixels represented as raw bytes
pub fn rgbs_to_bytes(mut v: Vec<RGB8>) -> Vec<u8> {
	unsafe {
		let new_len = v.len() * RGB_SIZE;
		v.set_len(new_len);
		mem::transmute(v)
	}
}

/// Convert the raw bytes of RGB8 pixels to actual RGB8 pixels
pub fn bytes_to_rgbs(v: Vec<u8>) -> Vec<RGB8> {
	unsafe {
		let new_len = v.len() / RGB_SIZE;
		let mut v_o: Vec<RGB8> = mem::transmute(v);
		v_o.set_len(new_len);
		v_o
	}
}

/// LED color smoothing function that does no smoothing
pub fn no_smooth(_: &RGB8, to: RGB8, _: f64) -> RGB8 {
	to
}

/// Linear smooth of LED colors with regards to time
pub fn linear_smooth(from: &RGB8, to: RGB8, factor: f64) -> RGB8 {
	if factor > 1.0 {
		to
	} else {
		let (r_diff, g_diff, b_diff) = (to.r as f64 - from.r as f64,
			to.g as f64 - from.g as f64,
			to.b as f64 - from.b as f64);
		RGB8{ r: (from.r as f64 + (r_diff * factor)) as u8,
			g: (from.g as f64 + (g_diff * factor)) as u8,
			b: (from.b as f64 + (b_diff * factor)) as u8,
		}
	}
}