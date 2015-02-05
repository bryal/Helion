#![feature(libc, core, std_misc, io, alloc)]

extern crate libc;

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

// B8G8R8A8 pixel size in bytes
static PIXEL_SIZE: usize = 4;

// Like the failed windows one
macro_rules! FAILED { ($hr:expr) => ($hr < 0) }

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

	fn average_color(&self, led: &Led) -> RGB8 {
		let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
		// Coordinate factors
		let &Led((x1f, y1f), (x2f, y2f)) = led;
		// Skip every second row and column for better performance, with not too much
		// signal loss for most monitors
		let (widthf32, heightf32) = ((self.width/2) as f32, (self.height/2) as f32);
		for row in (y1f * heightf32) as usize .. (y2f * heightf32) as usize {
			for col in (x1f * widthf32) as usize .. (x2f * widthf32) as usize {
				let i = 2 * PIXEL_SIZE * (row * self.width + col);
				b_sum += self.data[i] as u64;
				g_sum += self.data[i+1] as u64;
				r_sum += self.data[i+2] as u64;
			}
		}

		let n64 = (self.width * self.height) as u64;
		RGB8{ red: (r_sum/n64) as u8, green: (g_sum/n64) as u8, blue: (b_sum/n64) as u8 }
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

	// Replaces the selfs frame, returning it. This is not a problem in and by itself, only,
	// the pointer for the vector in the frame is also used at other locations, the vector may
	// therefore be unexpectedly modified
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

// (x1, y1), (x2, y2)
struct Led((f32, f32), (f32, f32));

static LEDS: [Led; 32] = [
	Led((0.0000, 0.9219), (0.0600, 1.0000)),
	Led((0.0000, 0.8450), (0.0600, 0.9242)),
	Led((0.0000, 0.7681), (0.0600, 0.8473)),
	Led((0.0000, 0.6912), (0.0600, 0.7704)),
	Led((0.0000, 0.6142), (0.0600, 0.6935)),
	Led((0.0000, 0.5373), (0.0600, 0.6165)),
	Led((0.0000, 0.4604), (0.0600, 0.5396)),
	Led((0.0000, 0.3835), (0.0600, 0.4627)),
	Led((0.0000, 0.3065), (0.0600, 0.3858)),
	Led((0.0000, 0.2296), (0.0600, 0.3088)),
	Led((0.0000, 0.1527), (0.0600, 0.2319)),
	Led((0.0000, 0.0758), (0.0600, 0.1550)),
	Led((0.0000, 0.0000), (0.0600, 0.0781)),
	Led((0.0000, 0.0000), (0.0600, 0.1000)),
	Led((0.0000, 0.0000), (0.0461, 0.1000)),
	Led((0.0448, 0.0000), (0.0916, 0.1000)),
	Led((0.0902, 0.0000), (0.1370, 0.1000)),
	Led((0.1357, 0.0000), (0.1825, 0.1000)),
	Led((0.1811, 0.0000), (0.2280, 0.1000)),
	Led((0.2266, 0.0000), (0.2734, 0.1000)),
	Led((0.2720, 0.0000), (0.3189, 0.1000)),
	Led((0.3175, 0.0000), (0.3643, 0.1000)),
	Led((0.3630, 0.0000), (0.4098, 0.1000)),
	Led((0.4084, 0.0000), (0.4552, 0.1000)),
	Led((0.4539, 0.0000), (0.5007, 0.1000)),
	Led((0.4993, 0.0000), (0.5461, 0.1000)),
	Led((0.5448, 0.0000), (0.5916, 0.1000)),
	Led((0.5902, 0.0000), (0.6370, 0.1000)),
	Led((0.6357, 0.0000), (0.6825, 0.1000)),
	Led((0.6811, 0.0000), (0.7280, 0.1000)),
	Led((0.7266, 0.0000), (0.7734, 0.1000)),
	Led((0.7720, 0.0000), (0.8189, 0.1000)),
];

fn main() {
	use CaptureResult::*;
	init_dxgi();
	let mut capturer = Capturer::new().unwrap();

	let (width, height) = capturer.output_dimensions();
	println!("{} x {}", width, height);

	for _ in 0..600u16 {
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

		let out_vals: Vec<_> = LEDS.iter()
			.map(|led| {
				let child_frame = shared_frame.clone();
				Future::spawn(move || child_frame.average_color(led)) })
			.collect::<Vec<_>>()
			.into_iter()
			.map(|mut guard| guard.get())
			.collect();

		unsafe { capturer.replace_frame(mem::replace(shared_frame.make_unique(),
			Frame::new())); }

		println!("{:?}", out_vals);
	}
}
