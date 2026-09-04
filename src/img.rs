#[cfg(target_arch = "x86_64")]
mod x86_64;

use super::*;

use fast_image_resize::{
    FilterType, Resizer, ResizeAlg, ResizeOptions,
    images::{TypedImage, TypedImageRef},
    pixels::U8x3,
};
use image::{ColorType, ImageDecoder, ImageFormat, ImageReader};

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

pub struct SurfaceLayout {
    pub width: u32,
    pub stride: usize,
    pub height: u32,
    pub size: usize,
}

impl SurfaceLayout {
    pub fn new(width: u32, height: u32) -> Self {
        let stride = width as usize * 4;
        let size = stride * height as usize;
        Self { width, stride, height, size }
    }
}

struct Functions {
    levels_to_bgra_fn: unsafe fn(&ColorTransform, &mut [u8], &[u8]),
    bgra_from_rgb_fn: unsafe fn(&mut [u8], &[u8]),
}

impl Functions {
    fn select(cpu_features: CpuFeatures) -> Self {
        #[cfg(target_arch = "x86_64")]
        return x86_64::select_fns(cpu_features);
        #[allow(unreachable_code)]
        Functions {
            levels_to_bgra_fn: levels_to_bgra,
            bgra_from_rgb_fn: bgra_from_rgb,
        }
    }

    fn levels_to_bgra(
        &self,
        transform: &ColorTransform,
        dst: &mut [u8],
        src: &[u8],
    ) {
        assert!(buf_is_aligned_to(src, 64));
        assert!(buf_is_aligned_to(dst, 64));
        unsafe { (self.levels_to_bgra_fn)(transform, dst, src) }
    }

    fn bgra_from_rgb(&self, dst: &mut [u8], src: &[u8]) {
        assert!(buf_is_aligned_to(src, 64));
        assert!(buf_is_aligned_to(dst, 64));
        unsafe { (self.bgra_from_rgb_fn)(dst, src) }
    }
}

pub struct WallpaperLoader {
    buf: Buf,
    buf_next: Buf,
    color_transform: Option<ColorTransform>,
    resizer: Resizer,
    resize_options: ResizeOptions,
    fns: Functions,
}

impl WallpaperLoader {
    pub fn new(
        cpu_features: CpuFeatures,
        color_transform: Option<ColorTransform>,
    ) -> Self {
        Self {
            buf: Buf::new(),
            buf_next: Buf::new(),
            color_transform,
            resizer: Resizer::new(),
            resize_options: ResizeOptions::new()
                .fit_into_destination(None)
                .resize_alg(ResizeAlg::Convolution(FilterType::Mitchell)),
            fns: Functions::select(cpu_features),
        }
    }

    pub fn load_wallpaper(
        &mut self,
        surface: &SurfaceLayout,
        buffer: &mut [u8],
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let Some(dst) = buffer.get_mut(..surface.size) else {
            bail!("Buffer size {} smaller than surface size {}",
                buffer.len(), surface.size);
        };
        let mut reader = ImageReader::new(Cursor::new(data));
        if let Ok(format) = ImageFormat::from_path(path) {
            reader.set_format(format);
        }
        reader = reader
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
            .with_context(|| format!(
                "Failed to initialize {file_format:?} image decoder"
            ))?;
        let (mut image_width, mut image_height) = decoder.dimensions();
        let image_size = decoder.total_bytes();
        let image_color_type = decoder.color_type();
        if image_width == 0 || image_height == 0
            || image_size > isize::MAX as u64
        {
            bail!("Image has invalid dimensions {image_width}x{image_height}")
        };
        let mut image_size = image_size as usize;
        debug!("Image {image_width}x{image_height} {image_color_type:?}");
        if image_color_type.has_alpha() {
            warn!("Image has alpha channel which will be ignored");
        }
        if let Ok(Some(_icc_profile)) = decoder.icc_profile() {
            debug!("Image has an embedded ICC color profile \
                but ICC color profile handling is not yet implemented");
        }
        let needs_resize = image_width != surface.width
            || image_height != surface.height;
        debug!("Decoding image into temporary buffer");
        self.buf.reserve(image_size);
        decoder.read_image(&mut self.buf[..image_size])
            .context("Failed to decode image")?;
        if image_color_type != ColorType::Rgb8 {
            debug!("Converting image color type from {:?} to Rgb8",
                image_color_type);
            let pixel_count = image_width as usize * image_height as usize;
            let image_size_next = pixel_count * 3;
            self.buf.reserve(image_size_next);
            match image_color_type {
                ColorType::L8 =>
                    rgb8_from_l8(&mut self.buf, pixel_count),
                ColorType::La8 =>
                    rgb8_from_la8(&mut self.buf, pixel_count),
                ColorType::Rgb8 => unreachable!(),
                ColorType::Rgba8 =>
                    rgb8_from_rgba8(&mut self.buf, pixel_count),
                ColorType::L16 =>
                    rgb8_from_l16(&mut self.buf, pixel_count),
                ColorType::La16 =>
                    rgb8_from_la16(&mut self.buf, pixel_count),
                ColorType::Rgb16 =>
                    rgb8_from_rgb16(&mut self.buf, pixel_count),
                ColorType::Rgba16 =>
                    rgb8_from_rgba16(&mut self.buf, pixel_count),
                ColorType::Rgb32F =>
                    rgb8_from_rgb32f(&mut self.buf, pixel_count),
                ColorType::Rgba32F =>
                    rgb8_from_rgba32f(&mut self.buf, pixel_count),
                _ => bail!("Unsupported color type {image_color_type:?}"),
            };
            image_size = image_size_next;
        }
        if needs_resize {
            debug!("Resizing image from {}x{} to {}x{}",
                image_width, image_height, surface.width, surface.height);
            let image_size_next =
                surface.width as usize * surface.height as usize * 3;
            self.buf_next.reserve(image_size_next);
            let src_image = TypedImageRef::<U8x3>::from_buffer(
                image_width,
                image_height,
                &self.buf[..image_size],
            ).unwrap();
            let mut dst_image = TypedImage::<U8x3>::from_buffer(
                surface.width,
                surface.height,
                &mut self.buf_next[..image_size_next],
            ).unwrap();
            self.resizer.resize_typed(
                &src_image,
                &mut dst_image,
                &self.resize_options,
            ).context("Failed to resize image")?;
            image_width = surface.width;
            image_height = surface.height;
            image_size = image_size_next;
            swap(&mut self.buf, &mut self.buf_next);
        }
        if let Some(transform) = &self.color_transform {
            debug!("Applying levels color transform to image");
            self.fns.levels_to_bgra(
                transform,
                dst,
                &self.buf[..image_size],
            );
        } else {
            self.fns.bgra_from_rgb(dst, &self.buf[..image_size]);
        }
        let _ = (image_width, image_height, image_size);
        Ok(())
    }
}

fn mulhi(left: u16, right: u16) -> u16 {
    (((left as u32) * (right as u32)) >> 16) as u16
}

fn levels_to_bgra(transform: &ColorTransform, dst: &mut [u8], src: &[u8]) {
    let pixel_count = dst.len() / 4;
    assert_eq!(src.len(), pixel_count * 3);
    assert_eq!(dst.len(), pixel_count * 4);
    unsafe {
        for i in 0..pixel_count {
            let r = *src.get_unchecked(3 * i    );
            let g = *src.get_unchecked(3 * i + 1);
            let b = *src.get_unchecked(3 * i + 2);
            *dst.get_unchecked_mut(4 * i    ) = transform.apply(b);
            *dst.get_unchecked_mut(4 * i + 1) = transform.apply(g);
            *dst.get_unchecked_mut(4 * i + 2) = transform.apply(r);
            *dst.get_unchecked_mut(4 * i + 3) = u8::MAX;
        }
    }
}

fn bgra_from_rgb(dst: &mut [u8], src: &[u8]) {
    let pixel_count = dst.len() / 4;
    assert_eq!(src.len(), pixel_count * 3);
    assert_eq!(dst.len(), pixel_count * 4);
    unsafe {
        for i in 0..pixel_count {
            let r = *src.get_unchecked(3 * i    );
            let g = *src.get_unchecked(3 * i + 1);
            let b = *src.get_unchecked(3 * i + 2);
            *dst.get_unchecked_mut(4 * i    ) = b;
            *dst.get_unchecked_mut(4 * i + 1) = g;
            *dst.get_unchecked_mut(4 * i + 2) = r;
            *dst.get_unchecked_mut(4 * i + 3) = u8::MAX;
        }
    }
}

fn rgb8_from_l8(buf: &mut [u8], pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(3).unwrap());
    unsafe {
        for i in (0..pixel_count).rev() {
            let l = *buf.get_unchecked(i);
            *buf.get_unchecked_mut(3 * i    ) = l;
            *buf.get_unchecked_mut(3 * i + 1) = l;
            *buf.get_unchecked_mut(3 * i + 2) = l;
        }
    }
}

fn rgb8_from_la8(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(3).unwrap());
    unsafe {
        for i in (0..pixel_count).rev() {
            let l = *buf.get_unchecked(2 * i);
            *buf.get_unchecked_mut(3 * i    ) = l;
            *buf.get_unchecked_mut(3 * i + 1) = l;
            *buf.get_unchecked_mut(3 * i + 2) = l;
        }
    }
}

fn rgb8_from_rgba8(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(4).unwrap());
    unsafe {
        for i in 0..pixel_count {
            let r = *buf.get_unchecked(4 * i    );
            let g = *buf.get_unchecked(4 * i + 1);
            let b = *buf.get_unchecked(4 * i + 2);
            *buf.get_unchecked_mut(3 * i    ) = r;
            *buf.get_unchecked_mut(3 * i + 1) = g;
            *buf.get_unchecked_mut(3 * i + 2) = b;
        }
    }
}

fn rgb8_from_l16(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(3).unwrap());
    unsafe {
        for i in (0..pixel_count).rev() {
            let l = *buf.view_u16().get_unchecked(i);
            *buf.get_unchecked_mut(3 * i    ) = (l >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 1) = (l >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 2) = (l >> 8) as u8;
        }
    }
}

fn rgb8_from_la16(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(4).unwrap());
    unsafe {
        for i in 0..pixel_count {
            let l = *buf.view_u16().get_unchecked(2 * i);
            *buf.get_unchecked_mut(3 * i    ) = (l >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 1) = (l >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 2) = (l >> 8) as u8;
        }
    }
}

fn rgb8_from_rgb16(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(6).unwrap());
    unsafe {
        for i in 0..pixel_count {
            let r = *buf.view_u16().get_unchecked(3 * i    );
            let g = *buf.view_u16().get_unchecked(3 * i + 1);
            let b = *buf.view_u16().get_unchecked(3 * i + 2);
            *buf.get_unchecked_mut(3 * i    ) = (r >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 1) = (g >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 2) = (b >> 8) as u8;
        }
    }
}

fn rgb8_from_rgba16(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(8).unwrap());
    unsafe {
        for i in 0..pixel_count {
            let r = *buf.view_u16().get_unchecked(4 * i    );
            let g = *buf.view_u16().get_unchecked(4 * i + 1);
            let b = *buf.view_u16().get_unchecked(4 * i + 2);
            *buf.get_unchecked_mut(3 * i    ) = (r >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 1) = (g >> 8) as u8;
            *buf.get_unchecked_mut(3 * i + 2) = (b >> 8) as u8;
        }
    }
}

fn rgb8_from_rgb32f(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(12).unwrap());
    unsafe {
        for i in 0..pixel_count {
            let r = *buf.view_f32().get_unchecked(3 * i    );
            let g = *buf.view_f32().get_unchecked(3 * i + 1);
            let b = *buf.view_f32().get_unchecked(3 * i + 2);
            *buf.get_unchecked_mut(3 * i    ) = (r * 256.0) as u8;
            *buf.get_unchecked_mut(3 * i + 1) = (g * 256.0) as u8;
            *buf.get_unchecked_mut(3 * i + 2) = (b * 256.0) as u8;
        }
    }
}

fn rgb8_from_rgba32f(buf: &mut Buf, pixel_count: usize) {
    assert!(buf.len() >= pixel_count.checked_mul(16).unwrap());
    unsafe {
        for i in 0..pixel_count {
            let r = *buf.view_f32().get_unchecked(4 * i    );
            let g = *buf.view_f32().get_unchecked(4 * i + 1);
            let b = *buf.view_f32().get_unchecked(4 * i + 2);
            *buf.get_unchecked_mut(3 * i    ) = (r * 256.0) as u8;
            *buf.get_unchecked_mut(3 * i + 1) = (g * 256.0) as u8;
            *buf.get_unchecked_mut(3 * i + 2) = (b * 256.0) as u8;
        }
    }
}

fn buf_is_aligned_to(buf: &[u8], align: usize) -> bool {
    ptr_is_aligned_to(buf.as_ptr(), align)
}

fn ptr_is_aligned_to<T>(ptr: *const T, align: usize) -> bool {
    if !align.is_power_of_two() {
        panic!("is_aligned_to: align is not a power-of-two");
    }
    ptr.addr() & (align - 1) == 0
}
