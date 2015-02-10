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

// TODO: Saturation manipulation by converting to HSV

#![feature(libc, core, std_misc, io, alloc, unboxed_closures)]

extern crate libc;
extern crate "rustc-serialize" as rustc_serialize;
extern crate time;
extern crate "serial-rust" as serial;

use config::Led;

use std::ptr;
use std::mem;
use std::num::Float;
use std::ops::Drop;
use std::old_io::timer;
use std::time::Duration;
use std::sync::Arc;
use std::sync::Future;

use libc::{uint8_t, 
	c_void,
	size_t};

mod config;

#[link(name = "DXGCap")]
extern {
	fn init();

	fn create_dxgi_manager() -> *mut c_void;

	fn delete_dxgi_manager(dxgi_manager: *mut c_void);

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut size_t,
		height: *mut size_t);

	// Returns whether succeded
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

#[derive(Clone, Debug)]
struct RGB8 {
	red: u8,
	green: u8,
	blue: u8
}
impl RGB8 {
	fn modify(&mut self, f: &Fn<(u8, u8, u8), Output=(u8, u8, u8)>) -> &mut Self {
		let r = f(self.red, self.green, self.blue);
		self.red = r.0;
		self.green = r.1;
		self.blue = r.2;
		self
	}
}

#[derive(Clone)]
struct Frame {
	width: u64,
	height: u64,
	data: Vec<u8>,
}

impl Frame {
	fn new() -> Frame {
		Frame{ width: 0, height: 0, data: Vec::new() }
	}

	fn average_color(&self, led: Led, resize_width: u64, resize_height: u64) -> RGB8 {
		let (resize_width_ratio, resize_height_ratio) = (
			(self.width as f32 / resize_width as f32),
			(self.height as f32 / resize_height as f32));
		let (start_y, end_y, start_x, end_x) = (
			(led.vscan.minimum * resize_height as f32) as u64,
			(led.vscan.maximum * resize_height as f32) as u64,
			(led.hscan.minimum * resize_width as f32) as u64,
			(led.hscan.maximum * resize_width as f32) as u64);
		let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
		for row in start_y..end_y {
			for col in start_x..end_x {
				let i = (DXGI_PIXEL_SIZE as f32 *
					(row as f32 * resize_height_ratio * self.width as f32 +
						col as f32 * resize_width_ratio)) as usize;
				b_sum += self.data[i] as u64;
				g_sum += self.data[i+1] as u64;
				r_sum += self.data[i+2] as u64;
			}
		}

		let n_of_pixels = ((end_x - start_x) * (end_y - start_y)) as u64;
		RGB8{ red: (r_sum/n_of_pixels) as u8, green: (g_sum/n_of_pixels) as u8,
			blue: (b_sum/n_of_pixels) as u8 }
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
			Capturer{ dxgi_manager: manager,
				frame: Frame { width: 0, height: 0, data: Vec::new() }}
		}
	}

	fn output_dimensions(&self) -> (u64, u64) {
		let (mut width, mut height): (size_t, size_t) = (0, 0);
		unsafe { get_output_dimensions(self.dxgi_manager, &mut width, &mut height); }
		(width, height)
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
				let buffer_size = buffer_size as usize;
				if buffer.is_null() {
					CaptureResult::CrFail
				} else {
					let (width, height) = self.output_dimensions();
					self.frame = Frame{ width: width, height: height,
						data: unsafe {
							Vec::from_raw_parts(buffer, buffer_size,
								buffer_size) }};
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

// Higher gamma -> Same lights, darker darks
fn gamma(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
	(((r as f32 / 255.0).powf(1.6) * 255.0) as u8,
		((g as f32 / 255.0).powf(2.0) * 220.0) as u8,
		((b as f32 / 255.0).powf(1.9) * 235.0) as u8)
}

fn init_pixel_buffer(n_leds: u16) -> Vec<u8> {
	// Initialize the output led pixel buffer
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

	// Initiate windows stuff that DXGI through DXGCap requires.
	init_dxgi();

	// Parse the HyperCon json config
	let config = config::parse_config();
	let leds = config.leds.as_slice();
	// Dimensions to resize to when analyzing captured frame. Smaller image => faster averaging
	let (analyze_width, analyze_height) =
		(config.framegrabber.width, config.framegrabber.height);
	let fps_limit = config.framegrabber.frequency_Hz;

	// Create and open the serial connection to the LEDstream device, e.g. an arduino
	let mut serial_con =
		serial::Connection::new(config.device.output.clone(), config.device.rate);
	serial_con.open().unwrap();

	// Note, while the `leds.len()` returns a usize, the max supported leds in LEDstream is u16,
	// which is still alot, but if in the future someone comes with 65'535+ leds, this will
	// become a problem
	let mut out_pixel_buffer = init_pixel_buffer(leds.len() as u16);

	// Create the screen capturer
	let mut capturer = Capturer::new();
	let (width, height) = capturer.output_dimensions();

	println!("Capture dimensions: {} x {}", width, height);
	println!("Analyze dimensions: {} x {}", analyze_width, analyze_height);
	println!("Serial port: \"{}\", Baud rate: {}",
		config.device.output.clone(), config.device.rate);
	println!("Number of leds: {}", leds.len());
	println!("Capture interval: ms: {}, fps: {}", 1000.0 / fps_limit, fps_limit);

	let mut last_frame_time = time::precise_time_s();
	loop {
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

		// Temporarily take ownership of the frame so we can Arc it for the following
		// multithreading with Futures
		let frame = unsafe { capturer.replace_frame(Frame::new()) };
		let mut shared_frame = Arc::new(frame);

		// Clear the led data from out_pixel_buffer, then populate with new pixels
		out_pixel_buffer.truncate(OUT_HEADER_SIZE);
		for pixel in leds.iter()
			.map(|led| {
				let led = led.clone();
				let child_frame = shared_frame.clone();
				Future::spawn(move ||child_frame.average_color(led, analyze_width,
					analyze_height)) })
			.collect::<Vec<_>>()
			.into_iter()
			.map(|mut guard| guard.get())
			.map(|mut rgb| { rgb.modify(&gamma); rgb })
		{
			out_pixel_buffer.push(pixel.red);
			out_pixel_buffer.push(pixel.green);
			out_pixel_buffer.push(pixel.blue);
		}

		// Write the pixel buffer to the arduino
		serial_con.write(out_pixel_buffer.as_slice());

		// Return the frame to its rightful owner. This is required since the pointer in
		// Frame.data is used unsafely by dxgi
		unsafe {
			capturer.replace_frame(
				mem::replace(shared_frame.make_unique(), Frame::new()));
		}

		// Limit the framerate to `fps_limit`. If current frame did not go overtime, sleep
		// so we won't go too fast.
		let now = time::precise_time_s();
		// `precise_time_s` will reset every now and then. To handle this, substitute
		// `delta_time` for zero if it is negative.
		let delta_time = now - last_frame_time;
		let overtime = -(delta_time.max(0.0) - 1.0 / fps_limit);
		if overtime > 0.0 {
			timer::sleep(Duration::microseconds((overtime * 1_000_000.0) as i64));
		}
		last_frame_time = time::precise_time_s();
	}
}
