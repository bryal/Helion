extern crate libc;

use std::ptr;
use std::ops::Drop;

use libc::{uint32_t, uint8_t, c_void};

type Byte = u8;

#[link(name = "DXGCap")]
extern {
	fn init();

	fn create_dxgi_manager() -> *mut c_void;

	fn delete_dxgi_manager(dxgi_manager: *mut c_void);

	fn get_output_dimensions(dxgi_manager: *const c_void, width: *mut uint32_t,
		height: *mut uint32_t);

	fn get_frame_bytes(dxgi_manager: *mut c_void, o_size: *mut uint32_t,
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

	fn dimensions(&self) -> (u32, u32) {
		let (mut width, mut height) = (0u32, 0u32);
		unsafe { get_output_dimensions(self.dxgi_manager, &mut width, &mut height); }
		(width, height)
	}

	fn get_frame(&mut self) -> &mut [Byte] {
		let mut buffer_size = 0u32;
		let mut buffer = ptr::null_mut::<Byte>();
		println!("2");

		//
		unsafe { get_frame_bytes(self.dxgi_manager, &mut buffer_size, &mut buffer); }
		//

		println!("3");
		self.current_frame = unsafe {
			Vec::from_raw_parts(buffer, buffer_size as usize, buffer_size as usize) };
		println!("4");
		self.current_frame.as_mut_slice()
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

	let mut capturer = Capturer::new().unwrap();

	let (width, height) = capturer.dimensions();
	println!("{} x {}", width, height);

	for i in 0..60u8 {
		println!("IN");
		capturer.get_frame();
		println!("OUT");
	}

}
