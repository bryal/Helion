#![feature(libc, core, std_misc, io, alloc)]

extern crate libc;
extern crate "rustc-serialize" as rustc_serialize;
extern crate time;

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

// B8G8R8A8, DXGI default, pixel size in bytes
static PIXEL_SIZE: usize = 4;

static FPS_CAP: f64 = 30.0;

static SKIP_PIXELS: usize = 3;

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
		let (widthf32, heightf32) = ((self.width/SKIP_PIXELS) as f32,
			(self.height/SKIP_PIXELS) as f32);
		let (start_y, end_y, start_x, end_x) = ((led.vscan.minimum * heightf32) as usize,
			(led.vscan.maximum * heightf32) as usize,
			(led.hscan.minimum * widthf32) as usize,
			(led.hscan.maximum * widthf32) as usize);
		for row in start_y..end_y {
			for col in start_x..end_x {
				let i = SKIP_PIXELS * PIXEL_SIZE * (row * self.width + col);
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
	fn new() -> Result<Capturer, ()> {
		let manager = unsafe { create_dxgi_manager() };
		if manager.is_null() {
			Err(())
		} else {
			Ok(Capturer{
				dxgi_manager: manager,
				frame: Frame { width: 0, height: 0, data: Vec::new() }})
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
	use CaptureResult::*;

	let config = config::parse_config();
	let leds = config.leds.as_slice();

	init_dxgi();

	let mut capturer = Capturer::new().unwrap();

	let (width, height) = capturer.output_dimensions();
	println!("{} x {}", width, height);

	let mut last_frame_time = time::precise_time_s();
	for _ in 0..300u16 {
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

		let frame = unsafe { capturer.replace_frame(Frame::new()) };

		let mut shared_frame = Arc::new(frame);

		let out_vals: Vec<_> = leds.iter()
			.map(|led| {
				let led = led.clone();
				let child_frame = shared_frame.clone();
				Future::spawn(move || child_frame.average_color(led)) })
			.collect::<Vec<_>>()
			.into_iter()
			.map(|mut guard| guard.get())
			.collect();

		unsafe {
			capturer.replace_frame(mem::replace(shared_frame.make_unique(),
			Frame::new()));
		}

		let delta_time = time::precise_time_s() - last_frame_time;
		let time_diff = delta_time - 1.0 / FPS_CAP;
		if time_diff < 0.0 {
			timer::sleep(Duration::microseconds(((-time_diff) * 1_000_000.0) as i64));
		}
		last_frame_time = time::precise_time_s();
	}
}
