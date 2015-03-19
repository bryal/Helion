use config::Region;
use color::{RGB8, BGRA8};

use libc::{c_void,
	uint8_t,
	size_t};
use std::ptr;
use std::mem;

/// Enum of results returned by DXGCap
#[repr(C)]
#[allow(dead_code)]
enum DXGCaptureResult {
	Ok,
	// Could not duplicate output, access denied. Might be in protected fullscreen.
	AccessDenied,
	// Access to the duplicated output was lost. Likely, mode was changed e.g. window => full
	AccessLost,
	// Error when trying to refresh outputs after some failure.
	RefreshFailure,
	// AcquireNextFrame timed out.
	Timeout,
	// General/Unexpected failure
	Fail,
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

	fn refresh_output(dxgi_manager: *mut c_void) -> bool;

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut u32, height: *mut u32);

	// Returns DXGI status code, HRESULT
	fn get_frame_bytes(dxgi_manager: *mut c_void, o_size: *mut size_t,
		o_bytes: *mut *mut uint8_t) -> DXGCaptureResult;
}

/// Possible errors when capturing
#[allow(dead_code)]
pub enum CaptureError {
	// Could not duplicate output, access denied. Might be in protected fullscreen.
	AccessDenied,
	// Access to the duplicated output was lost. Likely, mode was changed e.g. window => full
	AccessLost,
	// Error when trying to refresh outputs after some failure.
	RefreshFailure,
	// AcquireNextFrame timed out.
	Timeout,
	// General/Unexpected failure
	Fail,
}
impl CaptureError {
	/// Try to represent the DXGCap capture result as a CaptureError
	fn from_dxgcapture_result(cr: DXGCaptureResult) -> Option<CaptureError> {
		use self::DXGCaptureResult as D;
		match cr {
			D::Ok => None,
			D::AccessDenied => Some(CaptureError::AccessDenied),
			D::AccessLost => Some(CaptureError::AccessLost),
			D::RefreshFailure => Some(CaptureError::RefreshFailure),
			D::Timeout => Some(CaptureError::Timeout),
			D::Fail => Some(CaptureError::Fail),
		}
	}
}

static DXGI_PIXEL_SIZE: u64 = 4; // BGRA8 => 4 bytes, DXGI default

/// Initiate windows stuff that DXGCap requires.
fn init_dxgi() {
	unsafe { init(); }
}
fn uninit_dxgi() {
	unsafe { uninit(); }
}

/// Representation of an image as a vector of 32-bit BGRA pixels,
/// coupled with image dimensions
#[derive(Clone)]
pub struct Image {
	width: u32, height: u32,
	pixels: Vec<BGRA8>,
}
impl Image {
	fn new() -> Image {
		Image{ width: 0, height: 0, pixels: Vec::new() }
	}
}

/// An analyzer with a slot for an image to be analyzed. `resize_width` and `resize_height` specify
/// what resolution to use when analyzing the image. Often this decides number of rows/cols to skip.
/// Analysis that may be done is such as calculating average color of a region of the image.
#[derive(Clone)]
pub struct ImageAnalyzer {
	image: Image,
	resize_width: u32, resize_height: u32,
}
impl ImageAnalyzer {
	pub fn new() -> ImageAnalyzer {
		ImageAnalyzer{ image: Image::new(), resize_width: 1, resize_height: 1 }
	}

	/// Swap the image slotted in the analyzer for a new one. Return the old one.
	pub fn swap_slotted(&mut self, new: Image) -> Image {
		mem::replace(&mut self.image, new)
	}

	/// Change the dimensions to work with when analyzing
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

	/// Calculate the average color of `self.image` for region given by `led`
	pub fn average_color(&self, led: &Region) -> RGB8 {
		if self.image.pixels.len() == 0 {
			RGB8{ r: 0, g: 0, b: 0 }
		} else {
			let (resize_width_ratio, resize_height_ratio) = (
				self.image.width as f32 / self.resize_width as f32,
				self.image.height as f32 / self.resize_height as f32);
			let (y1, y2, x1, x2) = (
				(led.vscan.minimum * self.resize_height as f32) as usize,
				(led.vscan.maximum * self.resize_height as f32) as usize,
				(led.hscan.minimum * self.resize_width as f32) as usize,
				(led.hscan.maximum * self.resize_width as f32) as usize);
			let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
			for row in y1..y2 {
				for col in x1..x2 {
					let pixel = &self.image.pixels[(
						row as f32 * resize_height_ratio *
						self.image.width as f32 +
						col as f32 * resize_width_ratio) as usize];
					r_sum += pixel.r as u64;
					g_sum += pixel.g as u64;
					b_sum += pixel.b as u64;
				}
			}

			let n_of_pixels = ((x2 - x1) * (y2 - y1)) as u64;
			RGB8{r: (r_sum/n_of_pixels) as u8,
				g: (g_sum/n_of_pixels) as u8,
				b: (b_sum/n_of_pixels) as u8 }
		}
	}
}

/// A screen capturer for capturing the contents of a monitor.
/// Currently this is not much more than a ffi wrapper for the DXGCap dxgi manager,
/// and, as such, does not support capturing many fullscreen games, especially directx ones.
pub struct Capturer {
	dxgi_manager: *mut c_void,
}
impl Capturer {
	/// Initialize DXGCap and construct a new `Capturer` with a new dxgi manager
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

	/// Specify the amount of time to wait for new frame before returning.
	pub fn set_timeout(&mut self, timeout: u32) {
		unsafe { set_timeout(self.dxgi_manager, timeout) }
	}

	/// Specify which monitor to capture from.
	/// A value of `0` results in capture of primary monitor.
	/// In windows, value of `cs` corresponds to monitor IDs in `Display\Screen Resolution`
	#[allow(dead_code)]
	pub fn set_capture_source(&mut self, cs: u16) {
		unsafe { set_capture_source(self.dxgi_manager, cs) }
	}

	/// Return index of current capture source
	#[allow(dead_code)]
	pub fn get_capture_source(&mut self) -> u16 {
		unsafe { get_capture_source(self.dxgi_manager) }
	}

	/// Manually refresh dxgi output adapters. May be needed if access to desktop duplication
	/// is lost, but DXGCap did not fix it automatically.
	pub fn refresh_output(&mut self) -> bool {
		unsafe { refresh_output(self.dxgi_manager) }
	}

	/// Get the display resolution of selected output device.
	pub fn get_display_resolution(&self) -> (u32, u32) {
		let (mut width, mut height) = (0, 0);
		unsafe { get_output_dimensions(self.dxgi_manager, &mut width, &mut height); }
		(width, height)
	}

	/// Capture a frame from the capture source, and return captured bytes as an `Image`.
	/// On success, return an `Image`, otherwise, return the CaptureResult indicating error type
	pub fn capture_frame(&mut self) -> Result<Image, CaptureError> {
		let mut shared_buf_size: size_t = 0;
		let mut shared_buf = ptr::null_mut::<u8>();
		
		let cr = unsafe{
			get_frame_bytes(self.dxgi_manager, &mut shared_buf_size, &mut shared_buf) };
		if let DXGCaptureResult::Ok = cr  {
			if shared_buf.is_null() {
				Err(CaptureError::Fail)
			} else {
				let n_pixels = (shared_buf_size / DXGI_PIXEL_SIZE) as usize;
				let mut pixel_buf: Vec<BGRA8> = Vec::with_capacity(n_pixels);

				unsafe {
					ptr::copy(pixel_buf.as_mut_ptr(),
						shared_buf as *const BGRA8,
						n_pixels);
					pixel_buf.set_len(n_pixels);
				}

				let (width, height) = self.get_display_resolution();

				Ok(Image{ width: width, height: height, pixels: pixel_buf })
			}
		} else {
			Err(CaptureError::from_dxgcapture_result(cr)
				.expect("DXGCaptureResult was not an error"))
		}
	}
}
impl Drop for Capturer {
	/// Manually delete the `dxgi_manager` and uninit the dxgi stuff inited on creation.
	fn drop(&mut self) {
		unsafe {
			delete_dxgi_manager(self.dxgi_manager);
			uninit_dxgi();
		}
	}
}