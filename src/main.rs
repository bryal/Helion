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

use std::ptr;
use std::mem;
use std::iter::{repeat, range_step};
use std::num::Float;
use std::ops::Drop;
use std::old_io::timer;
use std::time::Duration;
use std::sync::Future;
use std::cmp::max;
use libc::{uint8_t, 
	c_void,
	size_t};

use config::{Led, parse_led_indices};
use color::{RGB8, RgbTransformer, Pixel};

mod config;
mod color;

#[link(name = "DXGCap")]
extern {
	fn init();

	fn create_dxgi_manager() -> *mut c_void;

	fn delete_dxgi_manager(dxgi_manager: *mut c_void);

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut size_t,
		height: *mut size_t);

	// DXGI status code, HRESULT
	fn get_frame_bytes(dxgi_manager: *mut c_void, o_size: *mut size_t,
		o_bytes: *mut *mut uint8_t) -> uint8_t;
}

static DXGI_PIXEL_SIZE: u64      = 4; // BGRA8 => 4 bytes, DXGI default
static OUT_PIXEL_SIZE: usize     = 3; // RGB8 => 3 bytes, what LEDstream expects
static OUT_HEADER_SIZE: usize    = 6; // Magic word + led count + checksum, 6 bytes

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
		println!("ratio: {}", self.resize_width_ratio);
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
		// TODO: Precaltulate stuff
		let (start_y, end_y, start_x, end_x) = (
			(led.vscan.minimum * self.resize_height as f32) as usize,
			(led.vscan.maximum * self.resize_height as f32) as usize,
			(led.hscan.minimum * self.resize_width as f32) as usize,
			(led.hscan.maximum * self.resize_width as f32) as usize);
		let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
		for row in start_y..end_y {
			for col in start_x..end_x {
				let i = (row as f32 * self.resize_height_ratio * self.width as f32 +
					col as f32 * self.resize_width_ratio) as usize;
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

fn init_pixel_buffer(n_leds: u16) -> Vec<u8> {
	let mut buffer: Vec<u8> = repeat(0)
		.take(OUT_HEADER_SIZE + n_leds as usize * OUT_PIXEL_SIZE)
		.collect();

	// A special header / magic word is expected by the corresponding LED streaming code 
	// running on the Arduino. This only needs to be initialized once because the number of  
	// LEDs remains constant.
	// Magic word. This is the same magic word as the one in the arduino program LEDstream
	buffer[0] = 'A' as u8;
	buffer[1] = 'd' as u8;
	buffer[2] = 'a' as u8;
	// In the two below, not sure why the -1 in `(n_leds - 1)` is needed,
	// but that's how LEDstream on the Arduino expects it
	buffer[3] = ((n_leds - 1) >> 8) as u8;    // LED count high byte
	buffer[4] = ((n_leds - 1) & 0xff) as u8;  // LED count low byte
	buffer[5] = buffer[3] ^ buffer[4] ^ 0x55; // Checksum
	buffer
}

// Smoothing functions for color transitions
fn no_smooth(_: u8, to: u8, _: f64) -> u8 {
	to
}
fn linear_smooth(from: u8, to: u8, factor: f64) -> u8 {
	if factor > 1.0 {
		to
	} else {
		let diff = to as f64 - from as f64;
		from + (diff * factor) as u8
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

		let rgb_transformer = if !(transform_conf.red.is_default()
			&& transform_conf.green.is_default()
			&& transform_conf.red.is_default())
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

	let mut write_future = {
		let mut serial_con =
			serial::Connection::new(config.device.output.clone(), config.device.rate);
		serial_con.open().unwrap();

		// Note, while the `leds.len()` returns a usize, the max supported leds in LEDstream is u16,
		// which is still alot, but if in the future someone comes with 65'535+ leds, this will
		// become a problem
		let out_pixel_buf = init_pixel_buffer(leds.len() as u16);

		// In order for serial writing not to block, do the writing in a Future. This requires stuff
		// to be moved, so return them from the Future for reuse.
		// TODO: Consider using Thread::spawn and channels
		Future::from_value((serial_con, out_pixel_buf))
	};

	let mut capturer = Capturer::new();
	// Dimensions to resize to when processing captured frame. Smaller image => faster averaging
	let resize_dimensions = (config.framegrabber.width, config.framegrabber.height);
	capturer.frame.set_resize_dimensions(resize_dimensions);

	let fps_limit = config.framegrabber.frequency_Hz;

	println!("Capture dimensions: {:?}", capturer.get_output_dimensions());
	println!("Analyze dimensions: {:?}", resize_dimensions);
	println!("Serial port: \"{}\", Baud rate: {}",
		config.device.output.clone(), config.device.rate);
	println!("Number of leds: {}", leds.len());
	println!("Capture interval: ms: {}, fps: {}", 1000.0 / fps_limit, fps_limit);

	// Function to use when smoothing led colors
	let smooth = match config.color.smoothing.type_.as_slice() {
		"linear" => linear_smooth as fn(u8, u8, f64) -> u8,
		_ => no_smooth as fn(u8, u8, f64) -> u8,
	};
	let smooth_time_const = max(config.color.smoothing.time_ms, 1) as f64 / 1000.0;
	let mut last_frame_time = time::precise_time_s();
	let mut last_diag_time = last_frame_time;
	let mut diag_i = 0;
	loop {
		let diag_bcf = time::precise_time_s();
		match capturer.capture_frame() {
			CrOk => (),
			// Access Denied means we are probably in fullscreen app with restricted 
			// access, sleep until we have access again
			CrAccessDenied => {
				println!("Access Denied");
				timer::sleep(Duration::seconds(2))
			},
			// Access Lost is handled automatically in DXGCap, Timeout is no biggie,
			// Fail might be bad, but might also not.
			// For all of these, just reuse the previous frame
			CrAccessLost => println!("Access to desktop duplication lost"),
			CrTimeout => (),
			CrFail => println!("Unexpected failure when capturing screen"),
		}
		let diag_acf = time::precise_time_s();
		let diag_bap = diag_acf;

		let (mut serial_con, mut out_pixel_buf) = write_future.into_inner();

		let smooth_factor = (time::precise_time_s() - last_frame_time) / smooth_time_const;
		for (rgb_pixel, buf_i) in leds.iter()
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
			.zip(range_step(OUT_HEADER_SIZE, OUT_HEADER_SIZE + out_pixel_buf.len(), 3))	
		{
			out_pixel_buf[buf_i] =
				smooth(out_pixel_buf[buf_i], rgb_pixel.r, smooth_factor);
			out_pixel_buf[buf_i + 1] =
				smooth(out_pixel_buf[buf_i + 1], rgb_pixel.g, smooth_factor);
			out_pixel_buf[buf_i + 2] =
				smooth(out_pixel_buf[buf_i + 2], rgb_pixel.b, smooth_factor);
		}

		let diag_aap = time::precise_time_s();

		write_future = Future::spawn(move || {
			serial_con.write(out_pixel_buf.as_slice());
			(serial_con, out_pixel_buf)
		});

		// `precise_time_s` will reset every now and then. To handle this, substitute
		// `delta_time` for zero if it is negative.
		let delta_time = time::precise_time_s() - last_frame_time;
		let overtime = -(delta_time.max(0.0) - 1.0 / fps_limit);
		if overtime > 0.0 {
			timer::sleep(Duration::microseconds((overtime * 1_000_000.0) as i64));
		}
		last_frame_time = time::precise_time_s();

		diag_i += 1;
		if diag_i >= 60 {
			println!("cf fps: {}", 1.0 / (diag_acf-diag_bcf));
			println!("ap fps: {}", 1.0 / (diag_aap-diag_bap));
			println!("avg fps: {}\n", 1.0 / ((last_frame_time - last_diag_time) / 60.0));
			diag_i = 0;
			last_diag_time = last_frame_time;
		}
	}
}
