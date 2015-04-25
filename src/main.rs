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

#![feature(core)]

extern crate rustc_serialize as rustc_serialize;
extern crate time;
extern crate se_rs_ial as serial;
extern crate dxgcap;

use config::parse_led_indices;
use color::{RGB8,
	RgbTransformer,
	Pixel};
use capture::{Capturer, ImageAnalyzer };

use dxgcap::CaptureError;
use std::iter::repeat;
use std::sync::mpsc::{Sender,
	Receiver,
	channel};
use std::thread;
use std::cmp::{max, partial_max};

pub mod config;
pub mod color;
pub mod capture;

/// A special header is expected by the corresponding LED streaming code running on the Arduino.
/// This only needs to be initialized once since the number of LEDs remains constant.
fn new_pixel_buf_header(n_leds: u16) -> Vec<u8> {
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

/// A timer to track time passed between refreshes. Can for example be used to limit frame rate.
struct FrameTimer {
	before: f64,
	last_frame_dt: f64,
}
impl FrameTimer {
	fn new() -> FrameTimer {
		FrameTimer{ before: time::precise_time_s(), last_frame_dt: 0.0 }
	}

	/// For how long the previous frame lasted
	fn last_frame_dt(&self) -> f64 {
		self.last_frame_dt
	}

	/// Time passed since last tick
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

	/// An update/frame/refresh has occured; take the time.
	fn tick(&mut self) {
		let now = time::precise_time_s();
		self.last_frame_dt = partial_max(now - self.before, 0.0).unwrap_or(0.0);
		self.before = now;
	}
}

/// Initialize a thread for serial writing given a serial port, baud rate, header to write before
/// each data write, and buffer with the actual led color data.
fn init_write_thread(port: &str, baud_rate: u32, header: Vec<u8>, pixel_buf: Vec<RGB8>)
	-> (Sender<Vec<RGB8>>, Receiver<Vec<RGB8>>)
{
	use std::io::Write;

	let baud_rate = serial::BaudRate::from_u32(baud_rate).unwrap();
	let mut serial_con = serial::Connection::open(port, baud_rate).unwrap();

	let (from_write_thread_tx, from_write_thread_rx) = channel();
	let (to_write_thread_tx, to_write_thread_rx) = channel::<Vec<RGB8>>();
	
	thread::spawn(move || loop {
		let pixel_buf = color::rgbs_to_bytes(to_write_thread_rx.recv().unwrap());

		match serial_con.write(&header) {
			Ok(hn) if hn == header.len() => match serial_con.write(&pixel_buf)
			{
				Ok(bn) if bn == pixel_buf.len() => (),
				Ok(_) => println!("Failed to write all bytes of RGB data"),
				Err(e) => println!("Failed to write RGB data, {}", e)
			},
			Ok(_) => println!("Failed to write all bytes in header"),
			Err(e) => println!("Failed to write header, {}", e)
		}

		from_write_thread_tx.send(color::bytes_to_rgbs(pixel_buf)).unwrap();
	});
	to_write_thread_tx.send(pixel_buf).unwrap();

	(to_write_thread_tx, from_write_thread_rx)
}

fn main() {
	let config = config::parse_config();

	let leds: &[_] = &config.leds;

	let mut led_transformers_list: Vec<_> = repeat(Vec::with_capacity(2)).take(leds.len()).collect();

	// Add color transforms from config to each led in matching vec
	for transform_conf in config.color.transform.iter() {
		let hsv_transformer = if !transform_conf.hsv.is_default() {
			Some(&transform_conf.hsv)
		} else { None };

		let rgb_transformer = if !(transform_conf.red.is_default() &&
			transform_conf.green.is_default() &&
			transform_conf.red.is_default())
		{
			Some(RgbTransformer::new(transform_conf.red.clone(),
				transform_conf.green.clone(),
				transform_conf.blue.clone()))
		} else { None };

		for range in parse_led_indices(&transform_conf.leds, leds.len()).iter() {
			for transformers in led_transformers_list[range.clone()].iter_mut() {
				transformers.push((rgb_transformer.clone(), hsv_transformer));
			}
		}
	}

	// Do serial writing on own thread as to not block.
	let (write_thread_tx, write_thread_rx) = {
		// Header to write before led data
		let out_header = new_pixel_buf_header(leds.len() as u16);

		// Skeleton for the output led pixel buffer to write to arduino
		let out_pixels = repeat(RGB8{r: 0, g: 0, b: 0}).take(leds.len()).collect();

		init_write_thread(&config.device.output, config.device.rate, out_header, out_pixels)
	};
	
	let mut capturer = Capturer::new();
	let capture_frame_interval = 1.0 / config.framegrabber.frequency_Hz;
	capturer.set_timeout_ms((1_000.0 * capture_frame_interval) as u32);

	let mut frame_analyzer = ImageAnalyzer::new();
	frame_analyzer.set_resize_dimensions(
		(config.framegrabber.width, config.framegrabber.height));

	// Function to use when smoothing led colors
	type SmoothFn = fn(&RGB8, RGB8, f64) -> RGB8;
	let smooth = match config.color.smoothing.type_.as_ref() {
		"linear" => color::linear_smooth as SmoothFn,
		_ => color::no_smooth as SmoothFn,
	};

	// max w/ 1 to avoid future divide by zero
	let smooth_time_const = max(config.color.smoothing.time_ms, 1) as f64 / 1000.0;

	let led_refresh_interval = 1.0 / config.color.smoothing.update_frequency;

	println!("Helion - An LED streamer\n\
		Number of LEDs: {}\n\
		Resize resolution: {} x {}\n\
		Capture rate: {} fps\n\
		LED refresh rate: {} hz\n\
		Serial port: {}",
		leds.len(), config.framegrabber.width, config.framegrabber.height,
		config.framegrabber.frequency_Hz, 1.0 / smooth_time_const, config.device.output);

	let mut capture_timer = FrameTimer::new();
	let mut led_refresh_timer = FrameTimer::new();
	loop {
		led_refresh_timer.tick();

		// Don't capture new frame if going faster than frame limit,
		// but still proceed to smooth leds
		if capture_timer.dt_to_now() > capture_frame_interval {
			// If something goes wrong, last frame is reused
			match capturer.capture_frame() {
				Ok(frame) => { frame_analyzer.swap_slotted(frame); },
				// Access Denied means we are probably in fullscreen app with
				// restricted access, wait for access
				Err(CaptureError::AccessDenied) => {
					println!("Access Denied");
					thread::sleep_ms(2_000)
				},
				// Should be handled automatically in DXGCap
				Err(CaptureError::AccessLost) =>
					println!("Access to desktop duplication lost"),
				Err(CaptureError::RefreshFailure) => {
					println!("Refresh Failure");
					loop {
						if capturer.acquire_output_duplication().is_ok() {
							break
						} else {
							println!("Refresh Failure");
							thread::sleep_ms(2_000)
						}
					}
				},
				Err(CaptureError::Timeout) => (),
				Err(_) => println!("Unexpected failure when capturing screen"),
			}
			capture_timer.tick();
		}

		let mut out_pixels = write_thread_rx.recv().unwrap();

		let smooth_factor = led_refresh_timer.last_frame_dt() / smooth_time_const;
		for (to_pixel, pixel_in_buf) in leds.iter()
			.map(|led| frame_analyzer.average_color(&led))
			.zip(led_transformers_list.iter())
			.map(|(average_color, color_transformers)|
				color_transformers.iter().fold(Box::new(average_color) as Box<Pixel>,
					|mut acc_color, &(ref opt_rgb_tr, ref opt_hsv_tr)|
				{
					if let Some(rgb_tr) = opt_rgb_tr.as_ref() {
						acc_color = Box::new(acc_color.rgb_transform(rgb_tr))
							as Box<Pixel>;
					}
					if let Some(hsv_tr) = opt_hsv_tr.as_ref() {
						Box::new(acc_color.hsv_transform(hsv_tr)) as Box<Pixel>
					} else {
						acc_color
					}
				})
					.to_rgb())
			.zip(out_pixels.iter_mut())
		{
			*pixel_in_buf = smooth(pixel_in_buf, to_pixel, smooth_factor);
		}

		write_thread_tx.send(out_pixels).unwrap();

		let time_left = led_refresh_interval - led_refresh_timer.dt_to_now();
		if time_left > 0.0 {
			thread::sleep_ms(if time_left > 0.0 { time_left * 1_000.0 } else { 0.0 } as u32);
		}
	}
}