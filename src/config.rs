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

use rustc_serialize::{json, Decoder, Decodable};

// TODO: fix snake cases

/// type:       The type of the device or leds (known types for now are 'ws2801', 'ldp8806',
///             'lpd6803', 'sedu', 'adalight', 'lightpack', 'test' and 'none')
/// rate:       The baudrate of the output to the device
/// colorOrder: The order of the color bytes ('rgb', 'rbg', 'bgr', etc.).
struct DeviceConfig {
	type_: String,
	rate: u32,
	color_order: String
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
				color_order: match d.read_struct_field("colorOrder", 2us,
					Decodable::decode)
				{
					Ok(v) => v,
					Err(e) => return Err(e) }}))
	}
}

/// saturationGain: The gain adjustement of the saturation
/// valueGain:      The gain adjustement of the value
#[derive(RustcDecodable)]
struct HSV {
	saturationGain: f32,
	valueGain: f32,
}

/// threshold:  The minimum required input value for the channel to be on (else zero)
/// gamma:      The gamma-curve correction factor
/// blacklevel: The lowest possible value (when the channel is black)
/// whitelevel: The highest possible value (when the channel is white)
#[derive(RustcDecodable)]
struct ColorSettings {
	threshold: f32,
	gamma: f32,
	blacklevel: f32,
	whitelevel: f32
}

/// leds:           The indices (or index ranges) of the leds to which this color transform applies
///                 (eg '0-5, 9, 11, 12-17'). The indices are zero based.
/// hsv:            The manipulation in the Hue-Saturation-Value color domain
/// red/green/blue: The manipulation in the Red-Green-Blue color domain
#[derive(RustcDecodable)]
struct Transform {
	leds: String,
	hsv: HSV,
	red: ColorSettings,
	green: ColorSettings,
	blue: ColorSettings
}

/// type:            The type of smoothing algorithm ('linear' or 'none')
/// time_ms:         The time constant for smoothing algorithm in milliseconds
/// updateFrequency: The update frequency of the leds in Hz
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
#[derive(RustcDecodable)]
struct ColorsManipulation {
	transform: Vec<Transform>,
	smoothing: Smoothing
}

#[derive(RustcDecodable, Clone)]
struct LedAxisPosFactor {
	pub minimum: f32,
	pub maximum: f32
}

/// hscan: The fractional part of the image along the horizontal used for the averaging 
///        (minimum and maximum inclusive)
/// vscan: The fractional part of the image along the vertical used for the averaging 
///        (minimum and maximum inclusive)
#[derive(RustcDecodable, Clone)]
pub struct Led {
	pub hscan: LedAxisPosFactor,
	pub vscan: LedAxisPosFactor
}

/// Struct to contain config generated by HyperCon, The Hyperion deamon configuration file builder
#[derive(RustcDecodable)]
struct LedsConfig  {
	device: DeviceConfig,
	/// Color manipulation configuration used to tune the output colors to specific
	/// surroundings. The configuration contains a list of color-transforms.
	color: ColorsManipulation,
	/// The configuration for each individual led. This contains the specification of the area 
	/// averaged of an input image for each led to determine its color.
	pub leds: Vec<Led>
}

pub fn parse_config() -> LedsConfig {
	let json = String::from_utf8(
		File::open(&Path::new("hyperion.config.json")).read_to_end().unwrap()).unwrap();
	let no_comments = json.lines()
		.map(|l| l.trim())
		.filter(|l| !l.is_empty() && !l.starts_with("//"))
		.collect::<String>();
	json::decode::<LedsConfig>(no_comments.as_slice()).unwrap()
}