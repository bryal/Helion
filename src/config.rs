// The MIT License (MIT)
//
// Copyright (c) 2015 Johan Johansson
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

#![allow(dead_code, non_snake_case)]

use std::fs::File;
use std::ops::Range;
use std::path;

use rustc_serialize::{json, Decoder, Decodable};

/// Configuration of a serial output
#[derive(Clone, RustcDecodable)]
pub struct DeviceConfig {
	/// The baudrate of the output to the device
	pub rate: u32,
	/// The address of the serial output.
	///
	/// On Windows, this is something like `COM42`. On Linux, `/dev/ttys42`
	pub output: String,
}

/// Color configuration in the format of HSV, Hue Saturation Value
#[derive(RustcDecodable, Clone)]
pub struct HSV {
	/// The gain adjustement of the saturation
	pub saturationGain: f32,
	/// The gain adjustement of the value
	pub valueGain: f32,
}
impl HSV {
	/// Test if config values are same as default values
	pub fn is_default(&self) -> bool {
		self.saturationGain == 1.0 && self.valueGain == 1.0
	}
}

/// Color configuration for additive color models such as RGB
#[derive(RustcDecodable, Clone)]
pub struct AdditiveColorConf {
	/// The minimum required input value for the channel to be on (else zero)
	pub threshold: f32,
	/// The gamma-curve correction factor
	pub gamma: f32,
	/// The lowest possible value (when the channel is black)
	pub blacklevel: f32,
	/// The highest possible value (when the channel is white)
	pub whitelevel: f32
}
impl AdditiveColorConf {
	/// Test if config values are same as default values
	pub fn is_default(&self) -> bool {
		self.threshold == 0.0
			&& self.gamma == 1.0
			&& self.blacklevel == 0.0
			&& self.whitelevel == 1.0
	}
}

/// Transformation config of the colors of specific leds
#[derive(RustcDecodable, Clone)]
pub struct Transform {
	/// The indices (or index ranges) of the leds to which this color transform applies,
	/// e.g. `"0-5, 9, 11, 12-17"`.
	///
	/// Indices start from zero.
	pub leds: String,
	/// Color manipulation in the Hue-Saturation-Value color domain
	pub hsv: HSV,
	/// Manipulation red color in the RGB domain
	pub red: AdditiveColorConf,
	/// Manipulation green color in the RGB domain
	pub green: AdditiveColorConf,
	/// Manipulation blue color in the RGB domain
	pub blue: AdditiveColorConf
}

/// Led color smoothing config
#[derive(Clone)]
pub struct Smoothing {
	/// The type of smoothing algorithm ('linear' or 'none')
	pub type_: String,
	/// Time constant for the smoothing algorithm in milliseconds
	pub time_ms: u32,
	/// The update frequency of the leds in Hz
	pub update_frequency: f64
}
impl Decodable for Smoothing {
	fn decode<D: Decoder>(decoder: &mut D) -> Result<Smoothing, D::Error> {
        	decoder.read_struct("Smoothing", 3, |d|
        		Result::Ok(Smoothing{
        			type_: match d.read_struct_field("type", 0, Decodable::decode) {
					Ok(v) => v,
					Err(e) => return Err(e) },
				time_ms: match d.read_struct_field("time_ms", 1,
					Decodable::decode)
				{
					Ok(v) => v,
					Err(e) => return Err(e) },
				update_frequency: match d.read_struct_field("updateFrequency", 2,
					Decodable::decode)
				{
					Ok(v) => v,
					Err(e) => return Err(e) }}))
	}
}

/// Color manipulation config used to tune the output colors to specific surroundings. 
#[derive(RustcDecodable, Clone)]
pub struct ColorsManipulation {
	/// A list of color transforms
	pub transform: Vec<Transform>,
	/// Smoothing of the colors in the time domain
	pub smoothing: Smoothing
}

/// The fractional part of an image along an axis (minimum and maximum inclusive)
#[derive(RustcDecodable, Clone, Debug)]
pub struct LedAxisPos {
	pub minimum: f32,
	pub maximum: f32
}

/// The region of a monitor to capture for an led
#[derive(RustcDecodable, Clone, Debug)]
pub struct Region {
	/// The fractional part of the image along the horizontal axis
	pub hscan: LedAxisPos,
	/// The fractional part of the image along the vertical axis
	pub vscan: LedAxisPos
}

/// Frame grabbing conf
#[derive(RustcDecodable, Clone)]
pub struct FrameGrabConf {
	/// The width of the grabbed frames in pixels
	pub width: usize,
	/// The height of the grabbed frames in pixels
	pub height: usize,
	/// The frequency of the frame grab in Hz
	pub frequency_Hz: f64
}

/// Container of config generated by HyperCon
#[derive(RustcDecodable, Clone)]
pub struct LedsConfig {
	// Device configuration. This only really applies on the RPi, so might never be used.
	pub device: DeviceConfig,
	/// Color manipulation configuration used to tune the output colors to specific
	/// surroundings. The configuration contains a list of color-transforms.
	pub color: ColorsManipulation,
	/// The configuration for each individual led. This contains the specification of the area 
	/// averaged of an input image for each led to determine its color.
	pub leds: Vec<Region>,
	///  The configuration for the frame-grabber
	pub framegrabber: FrameGrabConf
}

/// Parse the HyperCon JSON config to useable struct
pub fn parse_config() -> LedsConfig {
	use std::io::Read;

	let mut json_str = String::with_capacity(10_000);
	match File::open(&path::Path::new("hyperion.config.json")) {
		Ok(mut file) => match file.read_to_string(&mut json_str) {
			Ok(_) => (),
			Err(e) => {
				println!("Error\nConfig could not be read\n");
				panic!("{}", e)
			}
		},
		Err(e) => {
			println!("Error\nConfig file `hyperion.config.json` could not be opened\n");
			panic!("{}", e)
		}
	}

	let no_comments = json_str.lines()
		.map(|l| l.trim())
		.filter(|l| !l.is_empty() && !l.starts_with("//"))
		.collect::<String>();

	match json::decode::<LedsConfig>(no_comments.as_slice()) {
		Ok(o) => o,
		Err(e) => {
			println!("Error\n\
				Config could not be parsed\n\
				There might be a missing field, make sure that the config contains \
				all required settings.\n\
				https://github.com/bryal/Helion");
			panic!("{}", e)
		}
	}
}

/// Parse string of comma separated indices or index ranges to vector of ranges
///
/// # Examples
/// ```
/// assert_eq!(parse_led_indices("3, 4-8, 0, 20-24", 10), vec![3..4, 4..9, 0..1, 20..25]);
/// ```
pub fn parse_led_indices(indices_str: &str, total_n_leds: usize) -> Vec<Range<usize>> {
	if indices_str == "*" {
		vec![0..total_n_leds]
	} else {
		indices_str.split(',')
			.map(|index_str| index_str.trim().split('-').collect::<Vec<_>>())
			.filter(|is| is.len() <= 2 && is.len() >= 1)
			.filter_map(|index_strs| match index_strs.len() {
				1 => if let Ok(i) = index_strs[0].parse::<usize>() {
					Some(i..(i + 1))
				} else {
					None
				},
				2 => if let (Ok(i), Ok(j)) = (index_strs[0].parse::<usize>(),
					index_strs[1].parse::<usize>())
				{
					Some(i..(j + 1))
				} else {
					None
				},
				_ => None
			})
			.collect()
	}
}

#[test]
fn parse_led_indices_test() {
	assert_eq!(parse_led_indices("3, 4-8, 0, 20-24", 10), vec![3..4, 4..9, 0..1, 20..25]);
	assert_eq!(parse_led_indices("*", 10), vec![0..10]);
	assert_eq!(parse_led_indices("0, 1 - 5", 10), vec![0..1]);
	assert_eq!(parse_led_indices("1-A", 10), vec![]);
}