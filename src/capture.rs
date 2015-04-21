use config::Region;
use color::RGB8;

use dxgcap::{ DXGIManager, CaptureError, BGRA8 };
use std::mem;

/// Representation of an image as a vector of BGRA8 pixels, coupled with image dimensions
#[derive(Clone)]
pub struct Image {
	width: usize, height: usize,
	pixels: Vec<BGRA8>,
}
impl Image {
	/// Construct a new, empty image
	fn new() -> Image {
		Image{ width: 0, height: 0, pixels: Vec::new() }
	}
}

/// An analyzer with a slot for an image to be analyzed.
///
/// `resize_width` and `resize_height` specify what resolution to use when analyzing the image.
/// Often this decides number of rows/cols to skip.
/// Analysis that may be done is such as calculating average color of a region of the image.
#[derive(Clone)]
pub struct ImageAnalyzer {
	image: Image,
	resize_width: usize, resize_height: usize,
}
impl ImageAnalyzer {
	/// Construct a new `ImageAnalyzer` with an empty image slotted and resize dimensions of 1
	pub fn new() -> ImageAnalyzer {
		ImageAnalyzer{ image: Image::new(), resize_width: 1, resize_height: 1 }
	}

	/// Swap the image slotted in the analyzer for a new one. Return the old one.
	pub fn swap_slotted(&mut self, new: Image) -> Image {
		mem::replace(&mut self.image, new)
	}

	/// Change the dimensions to work with when analyzing
	pub fn set_resize_dimensions(&mut self, (resize_width, resize_height): (usize, usize)) {
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

	/// Calculate the average color for a region of slotted image given by `led`
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
///
/// Currently this is not much more than a ffi wrapper for the DXGCap dxgi manager
/// and, as such, does not support capturing many fullscreen games, especially directx ones.
pub struct Capturer {
	dxgi_manager: DXGIManager,
}
impl Capturer {
	/// Initialize DXGCap and construct a new `Capturer` with a new dxgi manager
	pub fn new() -> Capturer {
		Capturer{ dxgi_manager: DXGIManager::new(100).unwrap() }
	}

	/// Specify the amount of time to wait for new frame before returning.
	pub fn set_timeout_ms(&mut self, timeout_ms: u32) {
		self.dxgi_manager.set_timeout_ms(timeout_ms)
	}

	/// Specify which monitor to capture from.
	/// A value of `0` results in capture of primary monitor.
	/// In windows, value of `cs` corresponds to monitor IDs in `Display\Screen Resolution`
	#[allow(dead_code)]
	pub fn set_capture_source_index(&mut self, cs: usize) {
		self.dxgi_manager.set_capture_source_index(cs)
	}

	/// Return index of current capture source
	#[allow(dead_code)]
	pub fn get_capture_source_index(&mut self) -> usize {
		self.dxgi_manager.get_capture_source_index()
	}

	/// Manually refresh dxgi output adapters. May be needed if access to desktop duplication
	/// is lost and DXGCap did not fix it automatically.
	pub fn acquire_output_duplication(&mut self) -> Result<(), ()> {
		self.dxgi_manager.acquire_output_duplication()
	}

	/// Capture a frame from the capture source. Convert and return captured bytes as an `Image`
	pub fn capture_frame(&mut self) -> Result<Image, CaptureError> {
		let (pixel_buf, (width, height)) = match self.dxgi_manager.capture_frame() {
			Ok(o) => o,
			Err(e) => return Err(e)
		};

		Ok(Image{ width: width, height: height, pixels: pixel_buf })
	}
}