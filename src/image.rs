#![allow(clippy::too_many_arguments)]

use std::{
    fs::read_dir,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{bail, Context};
use fast_image_resize::{
    FilterType, PixelType, Resizer, ResizeAlg, ResizeOptions,
    images::Image,
};
use image::{
    ColorType, DynamicImage, ImageBuffer, ImageDecoder, ImageReader, Pixel,
};
use log::{debug, error, warn};
use smithay_client_toolkit::reexports::client::protocol::wl_shm;

#[derive(Clone, Copy, PartialEq)]
pub struct Levels {
    pub input_min: f32,
    pub input_max: f32,
    pub output_min: f32,
    pub output_max: f32,
}

impl Levels {
    pub fn from_legacy(brightness: i32, contrast: f32) -> Self {
        // Functions from the image crate
        let max = u8::MAX as f32;
        let percent = ((100.0 + contrast) / 100.0).powi(2);
        let do_contrast = |input: u8| -> u8 {
            let c = input as f32;
            let d = ((c / max - 0.5) * percent + 0.5) * max;
            let e = d.clamp(0.0, max);
            e as u8
        };
        let do_brightness = |input: u8| -> u8 {
            (input as i32 + brightness).clamp(0, u8::MAX as i32) as u8
        };

        let contrast_clips = |input: u8| -> bool {
            let c = input as f32;
            let d = ((c / max - 0.5) * percent + 0.5) * max;
            let e = d.clamp(0.0, max);
            e != d
        };
        let do_brightness_inv = |output: u8| -> u8 {
            (output as i32 - brightness).clamp(0, u8::MAX as i32) as u8
        };
        let do_contrast_inv_min = |output: u8| -> u8 {
            let c = output as f32;
            let d = ((c / max - 0.5) / percent + 0.5) * max;
            let e = d.clamp(0.0, max);
            let input_est = e as u8;
            let input_est_low = input_est.saturating_sub(1);
            let input_est_high = input_est.saturating_add(2);
            for input in input_est_low..=input_est_high {
                if !contrast_clips(input) {
                    return input
                }
            }
            input_est
        };
        let do_contrast_inv_max = |output: u8| -> u8 {
            let c = (output as f32 + 1.0).clamp(0.0, max);
            let d = ((c / max - 0.5) / percent + 0.5) * max;
            let e = d.clamp(0.0, max);
            let input_est = e as u8;
            let input_est_low = input_est.saturating_sub(1);
            let input_est_high = input_est.saturating_add(2);
            for input in (input_est_low..=input_est_high).rev() {
                if !contrast_clips(input) {
                    return input
                }
            }
            input_est.saturating_add(1)
        };

        // y = do_brightness(do_contrast(x)) is monotonic in input
        // so 0 and 255 always maps to output_min and output_max
        let output_min = do_brightness(do_contrast(0));
        let output_max = do_brightness(do_contrast(255));
        // find input_min/max which maps to output_min/max without clipping
        let mut input_min = do_contrast_inv_min(do_brightness_inv(output_min));
        let mut input_max = do_contrast_inv_max(do_brightness_inv(output_max));
        if input_max < input_min {
            let mid = input_min.midpoint(input_max);
            input_min = mid;
            input_max = mid;
        }

        Self {
            input_min: input_min as f32 / 256.0,
            input_max: (input_max as f32 + 1.0) / 256.0,
            output_min: output_min as f32 / 256.0,
            output_max: (output_max as f32 + 1.0) / 256.0,
        }
    }
}

// Applying levels is just clamping and computing a linear function.
// We complicate it a lot here to optimize for x86 simd:
// - starting with 8 bit subpixel samples
// - move to the [0, ...] sample range by subtracting input_min with saturation
// - clamp to [0, input_rel_max] sample range by clipping at input_rel_max
// - unpack to 16-bit 8.8 fixed point values
//   place the sample value in the high 8 bits
//   add place the value 128 (0.5 in 8.8 fixed point) in the low 8 bits
//   to reconstruct from quantization in the [0.0, input_rel_max + 1.0] range
// - use unsigned 16-bit mulhi to scale with an 8.8 fixed point factor
//   the low 8 bits of the 16-bit result shall contain
//   a scaled 8-bit sample in the [0, output_rel_max] sample range
// - pack the low 8 bits to get the scaled 8-bit sample
// - invert the input if needed by xoring 0 or !0
//   this is branchless and allows the use of the unsigned multiplication above
// - move to the [output_min, output_max] sample range by adding output_off
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ColorTransform {
    input_min: u8,
    input_rel_max: u8,
    factor: u16, // 8.8 fixed point
    xor_term: u8,
    output_off: u8,
}

impl ColorTransform {
    pub fn from_levels(levels: Levels) -> Self {
        // Convert continuous values to sample values
        let mut input_min = (levels.input_min * 256.0 + 0.5) as u8;
        let mut input_max = (levels.input_max * 256.0 - 0.5) as u8;
        let (output_min_f, output_max_f, xor_term) =
        if levels.output_min <= levels.output_max {
            (levels.output_min, levels.output_max, 0)
        } else {
            (levels.output_max, levels.output_min, !0)
        };
        let mut output_min = (output_min_f * 256.0 + 0.5) as u8;
        let mut output_max = (output_max_f * 256.0 - 0.5) as u8;

        // Deal with the degenerate cases
        if input_max < input_min {
            let mid_f = levels.input_min.midpoint(levels.input_max);
            let mid = (mid_f * 256.0) as u8;
            input_min = mid;
            input_max = mid;
        }
        if output_max < output_min {
            let mid_f = levels.output_min.midpoint(levels.output_max);
            let mid = (mid_f * 256.0) as u8;
            output_min = mid;
            output_max = mid;
        }

        let input_rel_max = input_max - input_min;
        let output_rel_max = output_max - output_min;
        let input_range = input_rel_max as f32 + 1.0;
        let output_range = output_rel_max as f32 + 1.0;
        let range_ratio = output_range / input_range;
        let factor = (range_ratio * 256.0 + 0.5) as u16;
        let output_off = if xor_term == 0 {
            output_min
        } else {
            output_max.wrapping_sub(u8::MAX)
        };
        Self { input_min, input_rel_max, factor, xor_term, output_off }
    }

    fn apply(&self, input: u8) -> u8 {
        let half_clamped = input.saturating_sub(self.input_min);
        let clamped = half_clamped.min(self.input_rel_max);
        let unpacked = ((clamped as u16) << 8) + 128;
        let scaled = mulhi(unpacked, self.factor);
        let packed = scaled as u8;
        let maybe_inverted = packed ^ self.xor_term;
        maybe_inverted.wrapping_add(self.output_off)
    }
}

pub struct WallpaperFile {
    pub path: PathBuf,
    pub workspace: String,
    pub workspace_number: i32,
    pub canon_path: PathBuf,
    pub canon_modified: u128,
}

pub fn output_wallpaper_files(
    output_dir: &Path,
) -> anyhow::Result<Vec<WallpaperFile>> {
    let dir = read_dir(output_dir).context("Failed to read directory")?;
    let mut ret = Vec::new();
    for dir_entry_result in dir {
        let dir_entry = match dir_entry_result {
            Ok(dir_entry) => dir_entry,
            Err(e) => {
                error!("Failed to read directory entries: {e}");
                break
            }
        };
        let path = dir_entry.path();
        if path.is_dir() {
            warn!("Skipping nested directory {path:?}");
            continue
        }
        let workspace = path.file_stem().unwrap()
            .to_string_lossy().into_owned();
        let workspace_number: i32 = workspace.parse().unwrap_or_default();
        let canon_path = match path.canonicalize() {
            Ok(canon_path) => canon_path,
            Err(e) => {
                error!("Failed to resolve absolute path for {path:?}: {e}");
                continue
            }
        };
        let canon_metadata = match canon_path.metadata() {
            Ok(canon_metadata) => canon_metadata,
            Err(e) => {
                error!("Failed to get file metadata for {canon_path:?}: {e}");
                continue
            }
        };
        let canon_modified = canon_metadata.modified().unwrap()
            .duration_since(UNIX_EPOCH).unwrap()
            .as_nanos();
        ret.push(WallpaperFile {
            path,
            workspace,
            workspace_number,
            canon_path,
            canon_modified,
        });
    }
    Ok(ret)
}

pub fn load_wallpaper(
    path: &Path,
    buffer: &mut [u8],
    surface_width: u32,
    surface_height: u32,
    surface_stride: usize,
    surface_format: wl_shm::Format,
    color_transform: Option<ColorTransform>,
    resizer: &mut Resizer,
) -> anyhow::Result<()> {
    let surface_size = surface_stride * surface_height as usize;
    let Some(dst) = buffer.get_mut(..surface_size) else {
        bail!("Provided buffer size {} smaller than wallpaper image size {}",
            buffer.len(), surface_size);
    };
    let reader = ImageReader::open(path)
        .context("Failed to open image file")?
        .with_guessed_format()
        .context("Failed to read image file format")?;
    let file_format = reader.format()
        .context("Failed to determine image file format")?;
    if !file_format.can_read() {
        bail!("Unsupported image file format {file_format:?}")
    } else if !file_format.reading_enabled() {
        bail!("Application was compiled with support \
            for image file format {file_format:?} disabled")
    }
    let mut decoder = reader.into_decoder()
        .context("Failed to initialize image decoder")?;
    let (image_width, image_height) = decoder.dimensions();
    let image_size = decoder.total_bytes();
    let image_color_type = decoder.color_type();
    if image_width == 0 || image_height == 0 || image_size > isize::MAX as u64 {
        bail!("Image has invalid dimensions {image_width}x{image_height}")
    };
    debug!("Image {image_width}x{image_height} {image_color_type:?}");
    if image_color_type.has_alpha() {
        warn!("Image has alpha channel which will be ignored");
    }
    if let Ok(Some(_)) = decoder.icc_profile() {
        debug!("Image has an embedded ICC color profile \
            but ICC color profile handling is not yet implemented");
    }
    let needs_resize = image_width != surface_width
        || image_height != surface_height;
    let surface_row_len = surface_width as usize * 3;
    if !needs_resize
        && image_color_type == ColorType::Rgb8
        && surface_format == wl_shm::Format::Bgr888
        && color_transform.is_none()
        && surface_row_len == surface_stride
    {
        debug!("Decoding image directly to destination buffer");
        decoder.read_image(dst).context("Failed to decode image")?;
        return Ok(());
    }
    let image = DynamicImage::from_decoder(decoder)
        .context("Failed to decode image")?;
    let mut image = image.into_rgb8();
    if let Some(ct) = color_transform {
        for (_, _, pixel) in image.enumerate_pixels_mut() {
            pixel.apply(|subpixel| ct.apply(subpixel))
        }
    }
    if needs_resize {
        debug!("Resizing image from {}x{} to {}x{}",
            image_width, image_height,
            surface_width, surface_height
        );
        let src_image = Image::from_vec_u8(
            image_width,
            image_height,
            image.into_raw(),
            PixelType::U8x3,
        ).unwrap();
        let mut dst_image = Image::new(
            surface_width,
            surface_height,
            PixelType::U8x3,
        );
        resizer.resize(
            &src_image,
            &mut dst_image,
            &ResizeOptions::new()
                .fit_into_destination(None)
                .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
        ).context("Failed to resize image")?;
        image = ImageBuffer::from_raw(
            surface_width,
            surface_height,
            dst_image.into_vec()
        ).unwrap();
    }
    match surface_format {
        wl_shm::Format::Bgr888 => {
            if surface_row_len == surface_stride {
                dst.copy_from_slice(&image);
            } else {
                copy_pad_stride(
                    &image,
                    dst,
                    surface_row_len,
                    surface_stride,
                    surface_height as usize,
                );
            }
        },
        wl_shm::Format::Xrgb8888 => {
            swizzle_bgra_from_rgb(&image, dst);
        },
        _ => unreachable!(),
    }
    Ok(())
}

fn copy_pad_stride(
    src: &[u8],
    dst: &mut [u8],
    src_stride: usize,
    dst_stride: usize,
    height: usize,
) {
    for row in 0..height {
        dst[row * dst_stride..][..src_stride]
            .copy_from_slice(&src[row * src_stride..][..src_stride]);
    }
}

fn swizzle_bgra_from_rgb(src: &[u8], dst: &mut [u8]) {
    let pixel_count = dst.len() / 4;
    assert_eq!(src.len(), pixel_count * 3);
    assert_eq!(dst.len(), pixel_count * 4);
    unsafe {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            return bgra_from_rgb_avx2(src, dst, pixel_count)
        }
        bgra_from_rgb(src, dst, pixel_count)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn bgra_from_rgb_avx2(src: &[u8], dst: &mut [u8], pixel_count: usize) {
    unsafe { bgra_from_rgb(src, dst, pixel_count) }
}

unsafe fn bgra_from_rgb(src: &[u8], dst: &mut [u8], pixel_count: usize) {
    unsafe {
        let mut src = src.as_ptr();
        let mut dst = dst.as_mut_ptr();
        for _ in 0..pixel_count {
            *dst.add(0) = *src.add(2); // B
            *dst.add(1) = *src.add(1); // G
            *dst.add(2) = *src.add(0); // R
            *dst.add(3) = u8::MAX;     // A
            src = src.add(3);
            dst = dst.add(4);
        }
    }
}

fn mulhi(left: u16, right: u16) -> u16 {
    (((left as u32) * (right as u32)) >> 16) as u16
}
