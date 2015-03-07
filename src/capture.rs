use config::Led;
use color::{RGB8, BGRA8};

use libc::{c_void,
	uint8_t,
	size_t};
use std::ptr;
use std::mem;

#[repr(C)]
#[allow(dead_code)]
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

#[link(name = "DXGCap")]
extern {
	fn init();

	fn uninit();

	fn create_dxgi_manager() -> *mut c_void;

	fn delete_dxgi_manager(dxgi_manager: *mut c_void);

	fn set_timeout(dxgi_manager: *mut c_void, timeout: u32);

	fn set_capture_source(dxgi_manager: *mut c_void, cs: u16);

	fn get_capture_source(dxgi_manager: *mut c_void) -> u16;

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut u32, height: *mut u32);

	// Returns DXGI status code, HRESULT
	fn get_frame_bytes(dxgi_manager: *mut c_void, o_size: *mut size_t,
		o_bytes: *mut *mut uint8_t) -> CaptureResult;
}

static DXGI_PIXEL_SIZE: u64 = 4; // BGRA8 => 4 bytes, DXGI default

/// Initiate windows stuff that DXGCap requires.
fn init_dxgi() {
	unsafe { init(); }
}
fn uninit_dxgi() {
	unsafe { uninit(); }
}

#[derive(Clone)]
struct Image {
	width: u32, height: u32,
	pixels: Vec<BGRA8>,
}
impl Image {
	fn new() -> Image {
		Image{ width: 0, height: 0, pixels: Vec::new() }
	}
}

#[derive(Clone)]
pub struct ImageAnalyzer {
	image: Image,
	resize_width: u32, resize_height: u32,
}
impl ImageAnalyzer {
	pub fn new() -> ImageAnalyzer {
		ImageAnalyzer{ image: Image::new(), resize_width: 0, resize_height: 0 }
	}

	pub fn swap_slotted(&mut self, new: Image) -> Image {
		mem::replace(&mut self.image, new)
	}

	pub fn set_resize_dimensions(&mut self, (resize_width, resize_height): (u32, u32)) {
		self.resize_width = if resize_width == 0 {
			self.image.width
		} else {
			resize_width
		};
		self.resize_height = if resize_height == 0 {
			self.image.height
		} else {
			resize_height
		};
	}

	pub fn average_color(&self, led: &Led) -> RGB8 {
		if self.image.pixels.len() == 0 {
			RGB8{ r: 0, g: 0, b: 0 }
		} else {
			let (resize_width_ratio, resize_height_ratio) = (
				self.image.width as f32 / self.resize_width as f32,
				self.image.height as f32 / self.resize_height as f32);
			let (start_y, end_y, start_x, end_x) = (
				(led.vscan.minimum * self.resize_height as f32) as usize,
				(led.vscan.maximum * self.resize_height as f32) as usize,
				(led.hscan.minimum * self.resize_width as f32) as usize,
				(led.hscan.maximum * self.resize_width as f32) as usize);
			let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
			for row in start_y..end_y {
				for col in start_x..end_x {
					let pixel = &self.image.pixels[(
						row as f32 * resize_height_ratio *
						self.image.width as f32 +
						col as f32 * resize_width_ratio) as usize];
					r_sum += pixel.r as u64;
					g_sum += pixel.g as u64;
					b_sum += pixel.b as u64;
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
}
impl Capturer {
	pub fn new() -> Capturer {
		init_dxgi();
		let manager = unsafe { create_dxgi_manager() };
		if manager.is_null() {
			uninit_dxgi();
			panic!("Unexpected null pointer when constructing Capturer.")
		} else {
			Capturer{ dxgi_manager: manager }
		}
	}

	pub fn set_timeout(&mut self, timeout: u32) {
		unsafe { set_timeout(self.dxgi_manager, timeout) }
	}

	#[allow(dead_code)]
	pub fn set_capture_source(&mut self, cs: u16) {
		unsafe { set_capture_source(self.dxgi_manager, cs) }
	}

	#[allow(dead_code)]
	pub fn get_capture_source(&mut self) -> u16 {
		unsafe { get_capture_source(self.dxgi_manager) }
	}

	pub fn get_device_resolution(&self) -> (u32, u32) {
		let (mut width, mut height) = (0, 0);
		unsafe { get_output_dimensions(self.dxgi_manager, &mut width, &mut height); }
		(width, height)
	}

	pub fn capture_frame(&mut self) -> Result<Image, CaptureResult> {
		let mut shared_buf_size: size_t = 0;
		let mut shared_buf = ptr::null_mut::<u8>();
		let cr = unsafe{
			get_frame_bytes(self.dxgi_manager, &mut shared_buf_size, &mut shared_buf) };
		if let CaptureResult::CrOk = cr  {
			if shared_buf.is_null() {
				Err(CaptureResult::CrFail)
			} else {
				let n_pixels = (shared_buf_size / DXGI_PIXEL_SIZE) as usize;
				let mut pixel_buf: Vec<BGRA8> = Vec::with_capacity(n_pixels);

				unsafe {
					ptr::copy(pixel_buf.as_mut_ptr(),
						shared_buf as *const BGRA8,
						n_pixels);
					pixel_buf.set_len(n_pixels);
				}

				let (width, height) = self.get_device_resolution();

				Ok(Image{ width: width, height: height, pixels: pixel_buf })
			}
		} else {
			Err(cr)
		}
	}
}
impl Drop for Capturer {
	fn drop(&mut self) {
		unsafe {
			delete_dxgi_manager(self.dxgi_manager);
			uninit_dxgi();
		}
	}
}