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

use std::old_io::File;
use std::ops::Range;

use rustc_serialize::{json, Decoder, Decodable};

/// type:       The type of the device or leds (known types for now are 'ws2801', 'ldp8806',
///             'lpd6803', 'sedu', 'adalight', 'lightpack', 'test' and 'none')
/// rate:       The baudrate of the output to the device
/// colorOrder: The order of the color bytes ('rgb', 'rbg', 'bgr', etc.).
#[derive(Clone)]
struct DeviceConfig {
	pub type_: String,
	pub rate: u32,
	pub output: String,
	pub color_order: String
}
impl Decodable for DeviceConfig {
	fn decode<D: Decoder>(decoder: &mut D) -> Result<DeviceConfig, D::Error> {
		decoder.read_struct("DeviceConfig", 3us, |d|
			Result::Ok(DeviceConfig{
				type_: match d.read_struct_field("type", 0us, Decodable::decode) {
					Ok(v) => v,
					Err(e) => return Err(e) },
				rate: match d.read_struct_field("rate", 1us, Decodable::decode) {
					Ok(v) => v,
					Err(e) => return Err(e) },
				output: match d.read_struct_field("output", 0us, Decodable::decode) {
					Ok(v) => v,
					Err(e) => return Err(e) },
				color_order: match d.read_struct_field("colorOrder", 2us,
					Decodable::decode)
				{
					Ok(v) => v,
					Err(e) => return Err(e) }}))
	}
}

/// saturationGain: The gain adjustement of the saturation
/// valueGain:      The gain adjustement of the value
#[derive(RustcDecodable, Clone)]
pub struct HSV {
	pub saturationGain: f32,
	pub valueGain: f32,
}
impl HSV {
	pub fn is_default(&self) -> bool {
		self.saturationGain == 1.0 && self.valueGain == 1.0
	}
}

/// threshold:  The minimum required input value for the channel to be on (else zero)
/// gamma:      The gamma-curve correction factor
/// blacklevel: The lowest possible value (when the channel is black)
/// whitelevel: The highest possible value (when the channel is white)
#[derive(RustcDecodable, Clone)]
pub struct ColorSettings {
	pub threshold: f32,
	pub gamma: f32,
	pub blacklevel: f32,
	pub whitelevel: f32
}
impl ColorSettings {
	pub fn is_default(&self) -> bool {
		self.threshold == 0.0
			&& self.gamma == 1.0
			&& self.blacklevel == 0.0
			&& self.whitelevel == 1.0
	}
}

/// leds:           The indices (or index ranges) of the leds to which this color transform applies
///                 (eg '0-5, 9, 11, 12-17'). The indices are zero based.
/// hsv:            The manipulation in the Hue-Saturation-Value color domain
/// red/green/blue: The manipulation in the Red-Green-Blue color domain
#[derive(RustcDecodable, Clone)]
pub struct Transform {
	pub leds: String,
	pub hsv: HSV,
	pub red: ColorSettings,
	pub green: ColorSettings,
	pub blue: ColorSettings
}

/// type:            The type of smoothing algorithm ('linear' or 'none')
/// time_ms:         The time constant for smoothing algorithm in milliseconds
/// updateFrequency: The update frequency of the leds in Hz
#[derive(Clone)]
struct Smoothing {
	type_: String,
	time_ms: u32,
	update_frequency: f32
}
impl Decodable for Smoothing {
	fn decode<D: Decoder>(decoder: &mut D) -> Result<Smoothing, D::Error> {
        	decoder.read_struct("Smoothing", 3us, |d|
        		Result::Ok(Smoothing{
        			type_: match d.read_struct_field("type", 0us, Decodable::decode) {
					Ok(v) => v,
					Err(e) => return Err(e) },
				time_ms: match d.read_struct_field("time_ms", 1us,
					Decodable::decode)
				{
					Ok(v) => v,
					Err(e) => return Err(e) },
				update_frequency: match d.read_struct_field("updateFrequency", 2us,
					Decodable::decode)
				{
					Ok(v) => v,
					Err(e) => return Err(e) }}))
	}
}

/// Color manipulation configuration used to tune the output colors to specific surroundings. 
/// transform: A list of color-transforms
/// smoothing: Smoothing of the colors in the time-domain        
#[derive(RustcDecodable, Clone)]
struct ColorsManipulation {
	pub transform: Vec<Transform>,
	pub smoothing: Smoothing
}

#[derive(RustcDecodable, Clone, Debug)]
struct LedAxisPosFactor {
	pub minimum: f32,
	pub maximum: f32
}

/// hscan: The fractional part of the image along the horizontal used for the averaging 
///        (minimum and maximum inclusive)
/// vscan: The fractional part of the image along the vertical used for the averaging 
///        (minimum and maximum inclusive)
#[derive(RustcDecodable, Clone, Debug)]
pub struct Led {
	pub hscan: LedAxisPosFactor,
	pub vscan: LedAxisPosFactor
}

/// width:        The width of the grabbed frames [pixels]
/// height:       The height of the grabbed frames [pixels]
/// frequency_Hz: The frequency of the frame grab [Hz]
#[derive(RustcDecodable, Clone)]
struct FrameGrabber {
	pub width: usize,
	pub height: usize,
	pub frequency_Hz: f64
}

/// Struct to contain config generated by HyperCon, The Hyperion deamon configuration file builder
#[derive(RustcDecodable, Clone)]
struct LedsConfig  {
	// Device configuration. This only really applies on the RPi, so might never be used.
	pub device: DeviceConfig,
	/// Color manipulation configuration used to tune the output colors to specific
	/// surroundings. The configuration contains a list of color-transforms.
	pub color: ColorsManipulation,
	/// The configuration for each individual led. This contains the specification of the area 
	/// averaged of an input image for each led to determine its color.
	pub leds: Vec<Led>,
	///  The configuration for the frame-grabber
	pub framegrabber: FrameGrabber
}

pub fn parse_config() -> LedsConfig {
	let js = match File::open(&Path::new("hyperion.config.json")).read_to_string() {
		Ok(o) => o,
		Err(e) => {
			println!("Error\nConfig file `hyperion.config.json` could not be opened\n");
			panic!("{}", e)
		}
	};
	let no_comments = js.lines()
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