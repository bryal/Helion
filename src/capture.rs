use config::Led;
use color::{RGB8, BGRA8};

use libc::{c_void,
	uint8_t,
	size_t};
use std::ptr;
use std::mem;

#[link(name = "DXGCap")]
extern {
	fn init();

	fn uninit();

	fn create_dxgi_manager() -> *mut c_void;

	fn delete_dxgi_manager(dxgi_manager: *mut c_void);

	fn set_timeout(dxgi_manager: *mut c_void, timeout: u32);

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut size_t,
		height: *mut size_t);

	// DXGI status code, HRESULT
	fn get_frame_bytes(dxgi_manager: *mut c_void, o_size: *mut size_t,
		o_bytes: *mut *mut uint8_t) -> uint8_t;
}

// Initiate windows stuff that DXGCap requires.
fn init_dxgi() {
	unsafe { init(); }
}

fn uninit_dxgi() {
	unsafe { uninit(); }
}

static DXGI_PIXEL_SIZE: u64 = 4; // BGRA8 => 4 bytes, DXGI default

pub enum CaptureResult {
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

	pub fn set_resize_dimensions(&mut self, (resize_width, resize_height): (usize, usize)) {
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

	pub fn average_color(&self, led: &Led) -> RGB8 {
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

pub struct Capturer {
	dxgi_manager: *mut c_void,
	pub frame: Frame,
}
impl Capturer {
	pub fn new() -> Capturer {
		init_dxgi();
		let manager = unsafe { create_dxgi_manager() };
		if manager.is_null() {
			uninit_dxgi();
			panic!("Unexpected null pointer when constructing Capturer.")
		} else {
			Capturer{ dxgi_manager: manager, frame: Frame::new() }
		}
	}

	pub fn set_timeout(&mut self, timeout: u32) {
		unsafe { set_timeout(self.dxgi_manager, timeout) }
	}

	pub fn get_output_dimensions(&self) -> (usize, usize) {
		let (mut width, mut height): (size_t, size_t) = (0, 0);
		unsafe { get_output_dimensions(self.dxgi_manager, &mut width, &mut height); }
		(width as usize, height as usize)
	}

	pub fn capture_frame(&mut self) -> CaptureResult {
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
			uninit_dxgi();
		}
	}
}