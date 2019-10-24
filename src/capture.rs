use captrs::Bgr8;
use color::Rgb8;
use config::Region;

/// An analyzer with a slot for an image to be analyzed.
///
/// `resize_width` and `resize_height` specify what resolution to use when analyzing the image.
/// Often this decides number of rows/cols to skip.
/// Analysis that may be done is such as calculating average color of a region of the image.
#[derive(Clone)]
pub struct ImageAnalyzer<'i> {
    pub data: &'i [Bgr8],
    pub width: usize,
    pub height: usize,
    resize_width: usize,
    resize_height: usize,
}

impl<'i> ImageAnalyzer<'i> {
    /// Construct a new `ImageAnalyzer` with an empty image slotted and resize dimensions of 1
    pub fn new(
        data: &'i [Bgr8],
        width: usize,
        height: usize,
        resize_width: usize,
        resize_height: usize,
    ) -> Self {
        let resize_width = if resize_width == 0 {
            width
        } else {
            resize_width
        };
        let resize_height = if resize_height == 0 {
            height
        } else {
            resize_height
        };
        Self {
            data: data,
            width: width,
            height: height,
            resize_width: resize_width,
            resize_height: resize_height,
        }
    }

    /// Calculate the average color for a region of slotted image given by `led`
    pub fn average_color(&self, led: Region) -> Rgb8 {
        if self.data.len() == 0 {
            Rgb8 { r: 0, g: 0, b: 0 }
        } else {
            let (resize_width_ratio, resize_height_ratio) = (
                self.width as f32 / self.resize_width as f32,
                self.height as f32 / self.resize_height as f32,
            );
            let (y1, y2, x1, x2) = (
                (led.vscan.minimum * self.resize_height as f32) as usize,
                (led.vscan.maximum * self.resize_height as f32) as usize,
                (led.hscan.minimum * self.resize_width as f32) as usize,
                (led.hscan.maximum * self.resize_width as f32) as usize,
            );
            let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
            for row in y1..y2 {
                for col in x1..x2 {
                    let i = row as f32 * resize_height_ratio * self.width as f32
                        + col as f32 * resize_width_ratio;

                    let pixel = &self.data[i as usize];

                    r_sum += pixel.r as u64;
                    g_sum += pixel.g as u64;
                    b_sum += pixel.b as u64;
                }
            }
            let n_of_pixels = ((x2 - x1) * (y2 - y1)) as u64;
            Rgb8 {
                r: (r_sum / n_of_pixels) as u8,
                g: (g_sum / n_of_pixels) as u8,
                b: (b_sum / n_of_pixels) as u8,
            }
        }
    }
}
