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

#![feature(libc, core, std_misc, io, alloc, unboxed_closures, box_syntax, path)]

extern crate libc;
extern crate "rustc-serialize" as rustc_serialize;
extern crate time;
extern crate "serial-rust" as serial;

use std::ptr;
use std::mem;
use std::iter::repeat;
use std::num::Float;
use std::ops::Drop;
use std::old_io::timer;
use std::time::Duration;
use std::sync::Arc;
use std::sync::Future;
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

#[derive(Clone)]
struct Frame {
	width: usize, height: usize,
	resize_width: usize, resize_height: usize,
	resize_width_ratio: f32, resize_height_ratio: f32,
	data: Vec<u8>,
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
				let i = (DXGI_PIXEL_SIZE as f32 *
					(row as f32 * self.resize_height_ratio * self.width as f32
						+ col as f32 * self.resize_width_ratio)) as usize;
				b_sum += self.data[i] as u64;
				g_sum += self.data[i+1] as u64;
				r_sum += self.data[i+2] as u64;
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
					// New buffer size, frame dimensions ahve changed
					let (width, height) = self.get_output_dimensions();
					self.frame.set_dimensions((width, height));
					self.frame.data = unsafe {
						Vec::from_raw_parts(buffer,
							buffer_size as usize,
							buffer_size as usize)
					};
					CaptureResult::CrOk
				}
			}
		} else {
			cr
		}
	}

	// Replaces the selfs frame, returning it. This is not a problem in and by itself, only
	// the pointer for the vector in the frame is also used at other locations, the vector may
	// therefore be unexpectedly modified. Remember to return replacement when done
	unsafe fn replace_frame(&mut self, replacement: Frame) -> Frame {
		mem::replace(&mut self.frame, replacement)
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
	let mut buffer: Vec<u8> =
		(0 .. (OUT_HEADER_SIZE + n_leds as usize * OUT_PIXEL_SIZE)).map(|_| 0).collect();

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

	let mut serial_con =
		serial::Connection::new(config.device.output.clone(), config.device.rate);
	serial_con.open().unwrap();

	// Note, while the `leds.len()` returns a usize, the max supported leds in LEDstream is u16,
	// which is still alot, but if in the future someone comes with 65'535+ leds, this will
	// become a problem
	let mut out_pixel_buffer = init_pixel_buffer(leds.len() as u16);

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
		
		// Temporarily take ownership of the frame so we can Arc it for the following
		// multithreading with Futures
		let frame = unsafe { capturer.replace_frame(Frame::new()) };
		let mut shared_frame = Arc::new(frame);

		out_pixel_buffer.truncate(OUT_HEADER_SIZE);
		for rgb_pixel in leds.iter()
			.map(|led| {
				let led = led.clone();
				let child_frame = shared_frame.clone();

				Future::spawn(move || child_frame.average_color(&led))
			})
			.zip(led_transformers.iter())
			.map(|(mut pixel_guard, transformers)| transformers.iter()
				.fold(box pixel_guard.get() as Box<Pixel>,
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
		{
			out_pixel_buffer.push(rgb_pixel.r);
			out_pixel_buffer.push(rgb_pixel.g);
			out_pixel_buffer.push(rgb_pixel.b);
		}

		let diag_aap = time::precise_time_s();
		let diag_bw = diag_aap;

		serial_con.write(out_pixel_buffer.as_slice());

		let diag_aw = time::precise_time_s();

		// Return the frame to its rightful owner. This is required since the pointer in
		// Frame.data is used unsafely by DXGCap
		unsafe {
			capturer.replace_frame(
				mem::replace(shared_frame.make_unique(), Frame::new()));
		}

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
			println!("aw fps: {}", 1.0 / (diag_aw-diag_bw));
			println!("avg fps: {}\n", 1.0 / ((last_frame_time - last_diag_time) / 60.0));
			diag_i = 0;
			last_diag_time = last_frame_time;
		}
	}
}
