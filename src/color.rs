use config::{self, AdditiveColorConf};
use partial_min;
use simd::f32x4;
use std::cmp::{max, min};
use std::slice;

static RGB_SIZE: usize = 3; // Rgb8 => 3 bytes, what LEDstream expects

/// Just a simple modulo function, since % in rust is remainder
fn modulo(l: f32, r: f32) -> f32 {
    if l >= 0.0 { l % r } else { r + l % r }
}

/// Describes how to transform the red, green, and blue in an RGB pixel
#[derive(Clone)]
pub struct RgbTransformer {
    pub r: AdditiveColorConf,
    pub g: AdditiveColorConf,
    pub b: AdditiveColorConf,
}
impl RgbTransformer {
    /// Transform a color with RGB modifiers.
    pub fn transform(&self, rgb: Rgb8) -> Rgb8 {
        let (mut channels, transformers) = ([rgb.r, rgb.g, rgb.b], [&self.r, &self.g, &self.b]);

        for (channel, transformer) in channels.iter_mut().zip(transformers.iter()) {
            let c = (*channel as f32 / 255.0).powf(transformer.gamma) * transformer.whitelevel *
                    (1.0 - transformer.blacklevel) +
                    transformer.blacklevel;
            *channel = (if c >= transformer.threshold { c } else { 0.0 } * 255.0) as u8;
        }
        Rgb8 {
            r: channels[0],
            g: channels[1],
            b: channels[2],
        }
    }
}

pub type HSVTransformer = config::HSV;
impl HSVTransformer {
    /// Transform the color of a pixel with HSV modifiers.
    pub fn transform(&self, hsv: HSV) -> HSV {
        HSV {
            hue: hsv.hue,
            saturation: partial_min(1.0, hsv.saturation * self.saturationGain, 1.0),
            value: partial_min(1.0, hsv.value * self.valueGain, 1.0),
        }
    }
}

/// RGB pixel with 8 bits per color.
#[derive(Clone, Copy, Debug)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
impl Rgb8 {
    pub fn to_hsv(&self) -> HSV {
        let max = max(max(self.r, self.g), self.b);
        let min = min(min(self.r, self.g), self.b);
        let chroma = max - min;

        let hue = 1.0 / 6.0 *
                  if chroma == 0 {
            0.0
        } else if max == self.r {
            modulo((self.g as f32 - self.b as f32) / chroma as f32, 6.0)
        } else if max == self.g {
            ((self.b as f32 - self.r as f32) / chroma as f32) + 2.0
        } else {
            ((self.r as f32 - self.g as f32) / chroma as f32) + 4.0
        };

        let value = max;

        let saturation = if value == 0 { 0.0 } else { chroma as f32 / value as f32 };

        HSV {
            hue: hue,
            saturation: saturation,
            value: value as f32 / 255.0,
        }
    }
}

/// A pixel in the [HSV](http://en.wikipedia.org/wiki/HSL_and_HSV) color format
#[derive(Clone, Debug)]
pub struct HSV {
    hue: f32,
    saturation: f32,
    value: f32,
}
impl HSV {
    pub fn to_rgb(&self) -> Rgb8 {
        if self.saturation == 0.0 {
            let v = (self.value * 255.0) as u8;
            Rgb8 { r: v, g: v, b: v }
        } else {
            let sector_f = self.hue * 6.0;
            let sector = sector_f as u8;
            let factorial_part = sector_f - sector as f32;
            let val = self.value * 255.0;

            let p = (val * (1.0 - self.saturation)) as u8;
            let q = (val * (1.0 - self.saturation * factorial_part)) as u8;
            let t = (val * (1.0 - self.saturation * (1.0 - factorial_part))) as u8;

            let val = val as u8;
            match sector {
                0 => {
                    Rgb8 { r: val, g: t, b: p }
                }
                1 => {
                    Rgb8 { r: q, g: val, b: p }
                }
                2 => {
                    Rgb8 { r: p, g: val, b: t }
                }
                3 => {
                    Rgb8 { r: p, g: q, b: val }
                }
                4 => {
                    Rgb8 { r: t, g: p, b: val }
                }
                _ => {
                    Rgb8 { r: val, g: p, b: q }
                }
            }
        }
    }
}

/// A pixel of any color format
pub enum Color {
    RGB(Rgb8),
    HSV(HSV),
}
impl Color {
    /// Convert the pixel to Rgb8
    pub fn into_rgb(self) -> Rgb8 {
        match self {
            Color::HSV(hsv) => hsv.to_rgb(),
            Color::RGB(rgb) => rgb,
        }
    }

    /// Convert the pixel to HSV
    pub fn into_hsv(self) -> HSV {
        match self {
            Color::RGB(rgb) => rgb.to_hsv(),
            Color::HSV(hsv) => hsv,
        }
    }
}

/// Represent Rgb8 colors as raw bytes
pub fn rgbs_as_bytes(v: &[Rgb8]) -> &[u8] {
    unsafe { slice::from_raw_parts(v.as_ptr() as *const _, v.len() * RGB_SIZE) }
}

/// LED color smoothing function that does no smoothing
pub fn no_smooth(_: Rgb8, to: Rgb8, _: f32) -> Rgb8 {
    to
}

/// Linear smooth of LED colors with regards to time
pub fn linear_smooth(from: Rgb8, to: Rgb8, factor: f32) -> Rgb8 {
    if factor > 1.0 {
        to
    } else {
        let from_f = f32x4::new(from.r as f32, from.g as f32, from.b as f32, 0.0);
        let to_f = f32x4::new(to.r as f32, to.g as f32, to.b as f32, 0.0);

        let diff = to_f - from_f;

        let res = from_f + diff * f32x4::splat(factor);

        Rgb8 {
            r: res.extract(0) as u8,
            g: res.extract(1) as u8,
            b: res.extract(2) as u8,
        }
    }
}
