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

#![feature(libc, core, std_misc, io, unboxed_closures, box_syntax, path)]

extern crate libc;
extern crate "rustc-serialize" as rustc_serialize;
extern crate time;
extern crate "serial-rust" as serial;

use config::{Led, parse_led_indices};
use color::{RGB8, RgbTransformer, Pixel};

use std::ptr;
use std::mem;
use std::iter::repeat;
use std::num::Float;
use std::ops::Drop;
use std::old_io::timer;
use std::time::Duration;
use std::sync::mpsc::channel;
use std::thread;
use std::cmp::{max, partial_max};
use libc::{uint8_t, 
	c_void,
	size_t};

mod config;
mod color;

#[link(name = "DXGCap")]
extern {
	fn init();

	fn create_dxgi_manager() -> *mut c_void;

	fn delete_dxgi_manager(dxgi_manager: *mut c_void);

	fn set_timeout(dxgi_manager: *mut c_void, timeout: u32);

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut size_t,
		height: *mut size_t);

	// DXGI status code, HRESULT
	fn get_frame_bytes(dxgi_manager: *mut c_void, o_size: *mut size_t,
		o_bytes: *mut *mut uint8_t) -> uint8_t;
}

static DXGI_PIXEL_SIZE: u64   = 4; // BGRA8 => 4 bytes, DXGI default

fn init_dxgi() {
	unsafe { init(); }
}

enum CaptureResult {
	CrOk,
	// Could not duplicate output, access denied. Might be in protected fullscreen.
	CrAccessDenied,
	// Access to the duplicated output was lost. Likely, mode was changed e.g. window => full
	CrAccessLost,
	// AcquireNextFrame timed out.
	CrTimeout,
	// General/Unexpected failure
	CrFail,
}

trait AsCaptureResult {
	fn as_capture_result(&self) -> CaptureResult;
}

impl AsCaptureResult for uint8_t {
	fn as_capture_result(&self) -> CaptureResult {
		match *self {
			0 => CaptureResult::CrOk,
			1 => CaptureResult::CrAccessDenied,
			2 => CaptureResult::CrAccessLost,
			3 => CaptureResult::CrTimeout,
			_ => CaptureResult::CrFail,
		}
	}
}

// Must contain only the fields [b, g, r, a] in that order, since this struct is transmuted to from
// DXGCap raw BGRA8 buffer.
#[derive(Clone)]
struct BGRA8 {
	b: u8,
	g: u8,
	r: u8,
	a: u8
}

#[derive(Clone)]
struct Frame {
	width: usize, height: usize,
	resize_width: usize, resize_height: usize,
	resize_width_ratio: f32, resize_height_ratio: f32,
	data: Vec<BGRA8>,
}

impl Frame {
	fn new() -> Frame {
		Frame{ width: 0, height: 0, resize_width: 0, resize_height: 0,
			resize_width_ratio: 0.0, resize_height_ratio: 0.0,
			data: Vec::new() }
	}

	fn update_process_ratio(&mut self) {
		self.resize_width_ratio = self.width as f32 / self.resize_width as f32;
		self.resize_height_ratio = self.height as f32 / self.resize_height as f32;
	}

	fn set_dimensions(&mut self, (width, height): (usize, usize)) {
		self.width = width;
		self.height = height;
		if self.resize_width == 0 {
			self.resize_width = self.width
		}
		if self.resize_height == 0 {
			self.resize_height = self.height
		}
		self.update_process_ratio();
	}

	fn set_resize_dimensions(&mut self, (resize_width, resize_height): (usize, usize)) {
		self.resize_width = if resize_width == 0 {
			self.width
		} else {
			resize_width
		};
		self.resize_height = if resize_height == 0 {
			self.height
		} else {
			resize_height
		};
		self.update_process_ratio();
	}

	fn average_color(&self, led: &Led) -> RGB8 {
		if self.data.len() == 0 {
			RGB8{ r: 0, g: 0, b: 0 }
		} else {
			let (start_y, end_y, start_x, end_x) = (
				(led.vscan.minimum * self.resize_height as f32) as usize,
				(led.vscan.maximum * self.resize_height as f32) as usize,
				(led.hscan.minimum * self.resize_width as f32) as usize,
				(led.hscan.maximum * self.resize_width as f32) as usize);
			let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
			for row in start_y..end_y {
				for col in start_x..end_x {
					let i = (row as f32 * self.resize_height_ratio * self.width as f32
						+ col as f32 * self.resize_width_ratio) as usize;
					r_sum += self.data[i].r as u64;
					g_sum += self.data[i].g as u64;
					b_sum += self.data[i].b as u64;
				}
			}

			let n_of_pixels = ((end_x - start_x) * (end_y - start_y)) as u64;
			RGB8{r: (r_sum/n_of_pixels) as u8,
				g: (g_sum/n_of_pixels) as u8,
				b: (b_sum/n_of_pixels) as u8 }
		}
	}
}

struct Capturer {
	dxgi_manager: *mut c_void,
	frame: Frame,
}

impl Capturer {
	fn new() -> Capturer {
		let manager = unsafe { create_dxgi_manager() };
		if manager.is_null() {
			panic!("Unexpected null pointer when constructing Capturer.")
		} else {
			Capturer{ dxgi_manager: manager, frame: Frame::new() }
		}
	}

	fn with_timeout(timeout: u32) -> Capturer {
		let mut c = Capturer::new();
		c.set_timeout(timeout);
		c
	}

	fn set_timeout(&mut self, timeout: u32) {
		unsafe { set_timeout(self.dxgi_manager, timeout) }
	}

	fn get_output_dimensions(&self) -> (usize, usize) {
		let (mut width, mut height): (size_t, size_t) = (0, 0);
		unsafe { get_output_dimensions(self.dxgi_manager, &mut width, &mut height); }
		(width as usize, height as usize)
	}

	fn capture_frame(&mut self) -> CaptureResult {
		let mut buffer_size: size_t = 0;
		let mut buffer = ptr::null_mut::<u8>();
		let cr = unsafe{
			get_frame_bytes(self.dxgi_manager, &mut buffer_size, &mut buffer)
				.as_capture_result() };
		if let CaptureResult::CrOk = cr  {
			if buffer as *const _ == self.frame.data.as_ptr() {
				CaptureResult::CrOk
			} else {
				if buffer.is_null() {
					CaptureResult::CrFail
				} else {
					// New buffer size, frame dimensions have changed
					let (width, height) = self.get_output_dimensions();
					self.frame.set_dimensions((width, height));
					// Raw bytes are bothersome, so cast them to BGRA8 structs.
					// BGRA8 contains only 4 fields, b, g, r, and a, and so,
					// this works fine.
					let bufsize = (buffer_size / DXGI_PIXEL_SIZE) as usize;
					self.frame.data = unsafe {
						Vec::from_raw_parts(buffer as *mut BGRA8,
							bufsize,
							bufsize)
					};
					CaptureResult::CrOk
				}
			}
		} else {
			cr
		}
	}
}

impl Drop for Capturer {
	fn drop(&mut self) {
		unsafe {
			let v = mem::replace(&mut self.frame.data, Vec::new());
			mem::forget(v);
			delete_dxgi_manager(self.dxgi_manager);
		}
	}
}

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
	use CaptureResult::*;

	// Initiate windows stuff that DXGCap requires.
	init_dxgi();

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
				let out_pixels = to_write_thread_rx.recv().unwrap();
				serial_con.write(out_header.as_slice());

				let out_bytes: Vec<u8> = color::rgbs_to_bytes(out_pixels);
				serial_con.write(out_bytes.as_slice());

				from_write_thread_tx.send(color::bytes_to_rgbs(out_bytes));
			}
		});
		to_write_thread_tx.send(out_pixels);

		(to_write_thread_tx, from_write_thread_rx)
	};
	

	let mut capturer = Capturer::new();
	let resize_dimensions = (config.framegrabber.width, config.framegrabber.height);
	capturer.frame.set_resize_dimensions(resize_dimensions);

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
		let diag_bcf = time::precise_time_s();

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

		let diag_acf = time::precise_time_s();
		let diag_bap = diag_acf;

		let mut out_pixels = write_thread_rx.recv().unwrap();

		let smooth_factor = led_refresh_timer.last_frame_dt() / smooth_time_const;
		for (to_pixel, out_buf_pixel) in leds.iter()
			.map(|led| {
				capturer.frame.average_color(&led)
			})
			.zip(led_transformers.iter())
			.map(|(pixel_future, transformers)| transformers.iter()
				.fold(box pixel_future as Box<Pixel>,
					|mut pixel, &(ref opt_rgb_tr, ref opt_hsv_tr)| {
						if let &Some(ref rgb_tr) = opt_rgb_tr {
							pixel = (box pixel.rgb_transform(rgb_tr))
								as Box<Pixel>;
						}
						if let &Some(ref hsv_tr) = opt_hsv_tr {
							(box pixel.hsv_transform(hsv_tr))
								as Box<Pixel>
						} else {
							pixel
						}
					})
				.to_rgb())
			.zip(out_pixels.iter_mut())
		{
			*out_buf_pixel = smooth(out_buf_pixel, to_pixel, smooth_factor);
		}

		let diag_aap = time::precise_time_s();

		write_thread_tx.send(out_pixels);
		
		let time_left = led_refresh_interval - led_refresh_timer.dt_to_now();
		if time_left > 0.0 {
			timer::sleep(Duration::microseconds((time_left * 1_000_000.0) as i64));
		}

		diag_i += 1;
		if diag_i >= 60 {
			diag_timer.tick();
			println!("cf fps: {}", 1.0 / (diag_acf-diag_bcf));
			println!("ap fps: {}", 1.0 / (diag_aap-diag_bap));
			println!("avg fps: {}\n", 1.0 / (diag_timer.last_frame_dt() / 60.0));
			diag_i = 0;
		}

		led_refresh_timer.tick();
	}
}
