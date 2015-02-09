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

#![feature(libc, core, std_misc, io, alloc)]

extern crate libc;
extern crate "rustc-serialize" as rustc_serialize;
extern crate time;
extern crate "serial-rust" as serial;

use config::Led;

use std::ptr;
use std::mem;
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

// pixel size in bytes, B8G8R8A8, DXGI default
static DXGI_PIXEL_SIZE: usize    = 4;
static FPS_CAP: f64              = 40.0;
static SKIP_PIXELS: f32          = 1.0;
static SERIAL_BAUD_RATE: u32     = 115200;
static SERIAL_PORT: &'static str = "COM3";

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

#[derive(Clone)]
struct Frame {
	width: usize,
	height: usize,
	data: Vec<u8>,
}

impl Frame {
	fn new() -> Frame {
		Frame{ width: 0, height: 0, data: Vec::new() }
	}

	fn average_color(&self, led: Led) -> RGB8 {
		let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
		// Skip every second row and column for better performance, with not too much
		// signal loss for most monitors
		let (widthf32, heightf32) = (self.width as f32 / SKIP_PIXELS,
			self.height as f32 / SKIP_PIXELS);
		let (start_y, end_y, start_x, end_x) = ((led.vscan.minimum * heightf32) as usize,
			(led.vscan.maximum * heightf32) as usize,
			(led.hscan.minimum * widthf32) as usize,
			(led.hscan.maximum * widthf32) as usize);
		for row in start_y..end_y {
			for col in start_x..end_x {
				let i = (SKIP_PIXELS * (DXGI_PIXEL_SIZE * (row * self.width + col)) as f32) as usize;
				// println!("b val: {}", self.data[i]);
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

	fn output_dimensions(&self) -> (usize, usize) {
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

fn init_dxgi() {
	unsafe { init(); }
}

fn main() {
	use std::io::Write;
	use CaptureResult::*;

	

	// Initiate windows stuff that DXGI through DXGCap requires.
	init_dxgi();

	// Create and open the serial connection to the LEDstream device, e.g. an arduino
	let mut serial_con = serial::Connection::new(SERIAL_PORT.to_string(), SERIAL_BAUD_RATE);
	serial_con.open().unwrap();

	// Parse the HyperCon json config
	let config = config::parse_config();
	let leds = config.leds.as_slice();

	// Initialize the output led pixel buffer
	// Note, while the `leds.len()` returns a usize, the max supported leds in LEDstream is u16,
	// which is still alot, but if in the future someone comes with 65'535+ leds, this will
	// become a problem
	let n_leds              = leds.len() as u16;
	let pixel_size          = 3; // RGBA8 => 3 bytes
	let header_size         = 6; // See header init right below
	let mut buffer: Vec<u8> =
		(0 .. (header_size + n_leds as usize * pixel_size)).map(|_| 0).collect();

	// A special header / magic word is expected by the corresponding LED streaming code 
	// running on the Arduino. This only needs to be initialized once because the number of  
	// LEDs remains constant.
	// Magic word. This is the same magic word as the one in the arduino program LEDstream
	buffer[0] = 'A' as u8;
	buffer[1] = 'd' as u8;
	buffer[2] = 'a' as u8;
	buffer[3] = ((n_leds - 1) >> 8) as u8;          // LED count high byte
	buffer[4] = ((n_leds - 1) & 0xff) as u8;        // LED count low byte
	buffer[5] = buffer[3] ^ buffer[4] ^ 0x55; // Checksum

	// Create the screen capturer
	let mut capturer = Capturer::new();

	let (width, height) = capturer.output_dimensions();
	println!("{} x {}", width, height);

	let mut last_frame_time = time::precise_time_s();
	loop {
		match capturer.capture_frame() {
			CrOk => (),
			// We are probably in fullscreen app with restricted access,
			// sleep until we have access again
			CrAccessDenied => timer::sleep(Duration::seconds(2)),
			// Has already been handeled in DXGCap, just try again
			CrAccessLost => continue,
			CrTimeout => continue,
			// CrFail. Might be bad, might be no big deal. Just ignore for now
			CrFail => continue,
		}

		// Temporarily take ownership of the frame so we can Arc it for the following
		// multithreading with Futures
		let frame = unsafe { capturer.replace_frame(Frame::new()) };
		let mut shared_frame = Arc::new(frame);

		// Clear the led data from buffer, then populate with new pixels
		buffer.truncate(header_size);
		for pixel in leds.iter()
			.map(|led| {
				let led = led.clone();
				let child_frame = shared_frame.clone();
				Future::spawn(move || child_frame.average_color(led)) })
			.collect::<Vec<_>>()
			.into_iter()
			.map(|mut guard| guard.get())
		{
			buffer.push(pixel.red);
			buffer.push(pixel.green);
			buffer.push(pixel.blue);
		}

		// Write the pixel buffer to the arduino
		serial_con.write(buffer.as_slice());

		// Return the frame to its rightful owner. This is required since the pointer in
		// Frame.data is used unsafely by dxgi
		unsafe {
			capturer.replace_frame(mem::replace(shared_frame.make_unique(),
				Frame::new()));
		}

		// Limit the framerate to `FPS_CAP`. If current frame did not go overtime, sleep
		// so we won't go too fast.
		let delta_time = time::precise_time_s() - last_frame_time;
		let overtime = delta_time - 1.0 / FPS_CAP;
		if overtime < 0.0 {
			timer::sleep(Duration::microseconds(((-overtime) * 1_000_000.0) as i64));
		}
		last_frame_time = time::precise_time_s();
	}
}
