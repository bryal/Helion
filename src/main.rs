extern crate libc;

use std::ptr;
use std::ops::Drop;

use libc::{uint32_t, uint8_t, c_void, size_t};

type Byte = u8;

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
	current_frame: Vec<Byte>,
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

	fn get_frame(&mut self) -> Result<&mut [Byte], ()> {
		let mut buffer_size: size_t = 0;
		let mut buffer = ptr::null_mut::<Byte>();
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

fn init_dxgi() {
	unsafe { init(); }
}

impl Drop for Capturer {
	fn drop(&mut self) {
		unsafe { delete_dxgi_manager(self.dxgi_manager); }
	}
}

fn main() {
	init_dxgi();
	{
		let mut capturer = Capturer::new().unwrap();

		let (width, height) = capturer.dimensions();
		println!("{} x {}", width, height);

		for i in 0..60u8 {
			println!("null frame: {}", capturer.get_frame().unwrap().iter().all(|b| *b == 0));
		}
		println!("Done1");
	}
	println!("Done2");

}
