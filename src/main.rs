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

// TODO: TESTS!

#![feature(libc, core, std_misc, io, unboxed_closures, box_syntax, path, old_io, fs)]

extern crate libc;
extern crate "rustc-serialize" as rustc_serialize;
extern crate time;
extern crate "serial-rust" as serial;

use config::parse_led_indices;
use color::{RGB8,
	RgbTransformer,
	Pixel};
use capture::Capturer;

use std::iter::repeat;
use std::old_io::timer;
use std::time::Duration;
use std::sync::mpsc::channel;
use std::thread;
use std::cmp::{max, partial_max};

mod config;
mod color;
mod capture;

fn new_pixel_buf_header(n_leds: u16) -> Vec<u8> {
	// A special header is expected by the corresponding LED streaming code 
	// running on the Arduino. This only needs to be initialized once because the number of  
	// LEDs remains constant.

	// In the two below, not sure why the -1 in `(n_leds - 1)` is needed,
	// but that's how LEDstream on the Arduino expects it
	let count_high = ((n_leds - 1) >> 8) as u8;  // LED count high byte
	let count_low = ((n_leds - 1) & 0xff) as u8; // LED count low byte
	vec![
		'A' as u8,
		'd' as u8,
		'a' as u8,

		count_high,
		count_low,

		count_high ^ count_low ^ 0x55, // Checksum
	]
}

struct FrameTimer {
	before: f64,
	last_frame_dt: f64,
}
impl FrameTimer {
	fn new() -> FrameTimer {
		FrameTimer{ before: time::precise_time_s(), last_frame_dt: 0.0}
	}

	fn last_frame_dt(&self) -> f64 {
		self.last_frame_dt
	}

	fn dt_to_now(&mut self) -> f64 {
		let now = time::precise_time_s();
		let dt = now - self.before;
		if dt >= 0.0 {
			dt
		} else {
			self.before = now;
			0.0
		}
	}

	fn tick(&mut self) {
		let now = time::precise_time_s();
		self.last_frame_dt = partial_max(now - self.before, 0.0).unwrap_or(0.0);
		self.before = now;
	}
}

fn main() {
	use std::io::Write;
	use capture::CaptureResult::*;

	// Parse the HyperCon json config
	let config = config::parse_config();
	let leds = config.leds.as_slice();

	let mut led_transformers: Vec<_> = repeat(Vec::with_capacity(2)).take(leds.len()).collect();

	// Add transforms from config to each led in matching vec
	for transform_conf in config.color.transform.iter() {
		let hsv_transformer = if !transform_conf.hsv.is_default() {
			Some(&transform_conf.hsv)
		} else { None };

		let rgb_transformer = if !(transform_conf.red.is_default() &&
			transform_conf.green.is_default() &&
			transform_conf.red.is_default())
		{
			Some(RgbTransformer{r: &transform_conf.red,
				g: &transform_conf.green,
				b: &transform_conf.blue})
		} else { None };

		for range in parse_led_indices(transform_conf.leds.as_slice(), leds.len()).iter() {
			for transforms in led_transformers[*range].iter_mut() {
				transforms.push((rgb_transformer.clone(), hsv_transformer));
			}
		}
	}

	// Do serial writing on own thread as to not block.
	let (write_thread_tx, write_thread_rx) = {
		let mut serial_con =
			serial::Connection::new(config.device.output.clone(), config.device.rate);
		serial_con.open().unwrap();

		// Header to write before led data
		let out_header = new_pixel_buf_header(leds.len() as u16);

		let out_pixels = repeat(RGB8{r: 0, g: 0, b: 0})
			.take(leds.len())
			.collect();

		let (from_write_thread_tx, from_write_thread_rx) = channel();
		let (to_write_thread_tx, to_write_thread_rx) = channel::<Vec<RGB8>>();
		
		thread::spawn(move || {
			loop {
				let out_bytes = color::rgbs_to_bytes(
					to_write_thread_rx.recv().unwrap());

				serial_con.write(out_header.as_slice())
					.and(serial_con.write(out_bytes.as_slice()))
					.unwrap_or_else(|e| {
						println!("Failed to write serial data\n\
							{}", e);
						0
					});

				from_write_thread_tx.send(color::bytes_to_rgbs(out_bytes)).unwrap();
			}
		});
		to_write_thread_tx.send(out_pixels).unwrap();

		(to_write_thread_tx, from_write_thread_rx)
	};
	
	let mut capturer = Capturer::new();
	capturer.frame.set_resize_dimensions(
		(config.framegrabber.width, config.framegrabber.height));

	let capture_frame_interval = 1.0 / config.framegrabber.frequency_Hz;
	capturer.set_timeout((1000.0 * capture_frame_interval) as u32);
	let led_refresh_interval = 1.0 / config.color.smoothing.update_frequency;

	// Function to use when smoothing led colors
	type SmoothFn = fn(&RGB8, RGB8, f64) -> RGB8;
	let smooth = match config.color.smoothing.type_.as_slice() {
		"linear" => color::linear_smooth as SmoothFn,
		_ => color::no_smooth as SmoothFn,
	};
	// max w/ 1 to avoid divide by zero
	let smooth_time_const = max(config.color.smoothing.time_ms, 1) as f64 / 1000.0;

	let mut capture_timer = FrameTimer::new();
	let mut led_refresh_timer = FrameTimer::new();
	let mut diag_timer = FrameTimer::new();
	let mut diag_i = 0;
	loop {
		led_refresh_timer.tick();

		// Don't capture frame if going faster than frame limit,
		// but still proceed to smooth leds
		if capture_timer.dt_to_now() > capture_frame_interval {
			// If something goes wrong, reuse last frame
			match capturer.capture_frame() {
				CrOk => (),
				// Access Denied means we are probably in fullscreen app with
				// restricted access, sleep until we have access again
				CrAccessDenied => {
					println!("Access Denied");
					timer::sleep(Duration::seconds(2))
				},
				// Should be handled automatically in DXGCap
				CrAccessLost => println!("Access to desktop duplication lost"),
				CrTimeout => (),
				CrFail => println!("Unexpected failure when capturing screen"),
			}
			capture_timer.tick();
		}

		let diag_bap = time::precise_time_s();

		let mut out_pixels = write_thread_rx.recv().unwrap();

		let smooth_factor = led_refresh_timer.last_frame_dt() / smooth_time_const;
		for (to_pixel, out_buf_pixel) in leds.iter()
			.map(|led| {
				capturer.frame.average_color(&led)
			})
			.zip(led_transformers.iter())
			.map(|(led_rgb, transformers)|
				transformers.iter().fold(box led_rgb as Box<Pixel>,
					|mut led_color, &(ref opt_rgb_tr, ref opt_hsv_tr)|
				{
					if let &Some(ref rgb_tr) = opt_rgb_tr {
						led_color = box led_color.rgb_transform(rgb_tr)
							as Box<Pixel>;
					}
					if let &Some(ref hsv_tr) = opt_hsv_tr {
						box led_color.hsv_transform(hsv_tr)
							as Box<Pixel>
					} else {
						led_color
					}
				})
				.to_rgb())
			.zip(out_pixels.iter_mut())
		{
			*out_buf_pixel = smooth(out_buf_pixel, to_pixel, smooth_factor);
		}

		let diag_aap = time::precise_time_s();

		write_thread_tx.send(out_pixels).unwrap();

		diag_i += 1;
		if diag_i >= 120 {
			diag_timer.tick();
			println!("ap fps: {}", 1.0 / (diag_aap-diag_bap));
			println!("avg fps: {}\n", 1.0 / (diag_timer.last_frame_dt() / 120.0));
			diag_i = 0;
		}

		let time_left = led_refresh_interval - led_refresh_timer.dt_to_now();
		if time_left > 0.0 {
			timer::sleep(Duration::microseconds((time_left * 1_000_000.0) as i64));
		}
	}
}