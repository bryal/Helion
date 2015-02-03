extern crate libc;

use std::ptr;
use std::mem;
use std::ops::Drop;

use libc::{uint32_t, uint8_t, c_void, size_t};

// B8G8R8A8 pixel size in bytes
static PIXEL_SIZE: u8 = 4;

#[link(name = "DXGCap")]
extern {
	fn init();

	fn create_dxgi_manager() -> *mut c_void;

	fn delete_dxgi_manager(dxgi_manager: *mut c_void);

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut size_t,
		height: *mut size_t);

	fn get_frame_bytes(dxgi_manager: *mut c_void, o_size: *mut size_t,
		o_bytes: *mut *mut uint8_t) -> bool;
}

struct Capturer {
	dxgi_manager: *mut c_void,
	current_frame: Vec<u8>,
}

impl Capturer {
	fn new() -> Result<Capturer, ()> {
		let manager = unsafe { create_dxgi_manager() };
		if manager.is_null() {
			Err(())
		} else {
			Ok(Capturer{
				dxgi_manager: manager,
				current_frame: Vec::new() })
		}
	}

	fn dimensions(&self) -> (usize, usize) {
		let (mut width, mut height): (size_t, size_t) = (0, 0);
		unsafe { get_output_dimensions(self.dxgi_manager, &mut width, &mut height); }
		(width as usize, height as usize)
	}

	fn get_frame(&mut self) -> Result<&mut [u8], ()> {
		let mut buffer_size: size_t = 0;
		let mut buffer = ptr::null_mut::<u8>();
		if unsafe { get_frame_bytes(self.dxgi_manager, &mut buffer_size, &mut buffer) } {
			if buffer_size == 0 {
				Ok(self.current_frame.as_mut_slice())
			} else {
				let buffer_size = buffer_size as usize;
				if buffer.is_null() {
					Err(())
				} else {
					self.current_frame = unsafe {
						Vec::from_raw_parts(buffer, buffer_size, buffer_size) };
					Ok(self.current_frame.as_mut_slice())
				}
			}
		} else {
			Err(())
		}
	}
}

impl Drop for Capturer {
	fn drop(&mut self) {
		unsafe {
			let v = mem::replace(&mut self.current_frame, Vec::new());
			mem::forget(v);
			delete_dxgi_manager(self.dxgi_manager);
		}
	}
}

fn average_color(pixels_bytes: &[u8]) -> (u64, u64, u64) {
	let n_pixels = pixels_bytes.len() as u64 / PIXEL_SIZE as u64;
	let (mut r_sum, mut b_sum, mut g_sum) = (0u64, 0u64, 0u64);
	for n in (0..n_pixels) {
		let i = n as usize * PIXEL_SIZE as usize;
		let (b, g, r) = (pixels_bytes[i], pixels_bytes[i+1], pixels_bytes[i+2]);
		r_sum += r as u64;
		b_sum += b as u64;
		g_sum += g as u64;
	}
	(r_sum/n_pixels, b_sum/n_pixels, g_sum/n_pixels)
}

fn init_dxgi() {
	unsafe { init(); }
}

fn main() {
	init_dxgi();
	let mut capturer = Capturer::new().unwrap();

	let (width, height) = capturer.dimensions();
	println!("{} x {}", width, height);

	for i in 0..240u16 {
		let frame = capturer.get_frame().unwrap();
		// println!("null frame: {}", frame.iter().all(|b| *b == 0));
		let avs = average_color(frame);
		println!("Avs: {} {} {}", avs.0, avs.1, avs.2);
	}
}
