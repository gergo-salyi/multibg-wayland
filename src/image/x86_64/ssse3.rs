use super::*;
use std::arch::x86_64::{
    __m128i as Simd,
    _mm_add_epi8 as add_epi8,
    _mm_alignr_epi8 as alignr_epi8,
    _mm_cvtsi128_si64 as cvtsi128_si64,
    _mm_load_si128,
    _mm_min_epu8 as min_epu8,
    _mm_or_si128 as or,
    _mm_set1_epi8 as set1_epi8,
    _mm_set1_epi16 as set1_epi16,
    _mm_shuffle_epi8 as shuffle_epi8,
    _mm_shuffle_epi32 as shuffle_epi32,
    _mm_store_si128,
    _mm_storeu_si64,
    _mm_subs_epu8 as subs_epu8,
    _mm_xor_si128 as xor,
};

const SIMD_SIZE: usize = size_of::<Simd>();

macro_rules! load_const {
    ($sym:ident) => {
        unsafe {
            let _ = const { let _type_check: Simd = $sym; };
            let out: Simd;
            asm!(
                "movdqa {}, xmmword ptr [rip+{}]",
                lateout(xmm_reg) out,
                sym $sym,
                options(pure, readonly, preserves_flags, nostack),
            );
            out
        }
    }
}

#[target_feature(enable = "sse,sse2,ssse3")]
pub unsafe fn levels_in_place(transform: &ColorTransform, buf: &mut [u8]) {
    let levels = Levels::from_color_tranform(transform);
    unsafe {
        let (vectors, suffix) = split_aligned_mut::<Simd>(buf);
        for vector_mut in vectors {
            let input = load(vector_mut);
            let output = levels.levels(input);
            store(vector_mut, output);
        }
        if suffix.is_empty() {
            return
        }
        let input = load_last(suffix);
        let output = levels.levels(input);
        store_last(suffix, output);
    }
}

#[target_feature(enable = "sse,sse2,ssse3")]
pub unsafe fn levels_to_bgra(
    transform: &ColorTransform,
    dst: &mut [u8],
    src: &[u8],
) {
    let levels = Levels::from_color_tranform(transform);
    let colors = BgraFromRgb::new();
    unsafe {
        let (src_chunks, src_suffix) = split_aligned::<[Simd; 3]>(src);
        let (dst_chunks, dst_suffix) = split_aligned_mut::<[Simd; 4]>(dst);
        for i in 0..dst_chunks.len() {
            let src_chunk = src_chunks.get_unchecked(i);
            let dst_chunk = dst_chunks.get_unchecked_mut(i);
            let mut rgb = load_array(src_chunk);
            rgb = rgb.map(|rgb| levels.levels(rgb));
            let bgra = colors.bgra_from_rgb(rgb);
            store_array(dst_chunk, bgra);
        }
        if dst_suffix.is_empty() {
            return
        }
        let mut rgb = load_last_array(src_suffix);
        rgb = rgb.map(|rgb| levels.levels(rgb));
        let bgra = colors.bgra_from_rgb(rgb);
        store_last_array(dst_suffix, bgra);
    }
}

#[target_feature(enable = "sse,sse2,ssse3")]
pub unsafe fn bgra_from_rgb(dst: &mut [u8], src: &[u8]) {
    let colors = BgraFromRgb::new();
    unsafe {
        let (src_chunks, src_suffix) = split_aligned::<[Simd; 3]>(src);
        let (dst_chunks, dst_suffix) = split_aligned_mut::<[Simd; 4]>(dst);
        for i in 0..dst_chunks.len() {
            let src_chunk = src_chunks.get_unchecked(i);
            let dst_chunk = dst_chunks.get_unchecked_mut(i);
            let rgb = load_array(src_chunk);
            let bgra = colors.bgra_from_rgb(rgb);
            store_array(dst_chunk, bgra);
        }
        if dst_suffix.is_empty() {
            return
        }
        let rgb = load_last_array(src_suffix);
        let bgra = colors.bgra_from_rgb(rgb);
        store_last_array(dst_suffix, bgra);
    }
}

#[derive(Clone, Copy)]
struct Levels {
    input_min: Simd,
    input_rel_max: Simd,
    bias: Simd,
    factor: Simd,
    xor_term: Simd,
    output_off: Simd,
}

static BIAS: Simd = unsafe { transmute([128u8 as i8; 16]) };

impl Levels {
    #[inline]
    #[target_feature(enable = "sse,sse2,ssse3")]
    fn from_color_tranform(transform: &ColorTransform) -> Self {
        Self {
            input_min: set1_epi8(transform.input_min as i8),
            input_rel_max: set1_epi8(transform.input_rel_max as i8),
            bias: load_const!(BIAS),
            factor: set1_epi16(transform.factor as i16),
            xor_term: set1_epi8(transform.xor_term as i8),
            output_off: set1_epi8(transform.output_off as i8),
        }
    }

    #[inline]
    #[target_feature(enable = "sse,sse2,ssse3")]
    fn levels(self, input: Simd) -> Simd {
        let half_clamped = subs_epu8(input, self.input_min);
        let clamped = min_epu8(half_clamped, self.input_rel_max);
        let unpacked_lo = unpacklo_epi8(self.bias, clamped);
        let unpacked_hi = unpackhi_epi8(self.bias, clamped);
        let scaled_lo = mulhi_epu16(unpacked_lo, self.factor);
        let scaled_hi = mulhi_epu16(unpacked_hi, self.factor);
        let packed = packus_epi16(scaled_lo, scaled_hi);
        let maybe_inverted = xor(packed, self.xor_term);
        add_epi8(maybe_inverted, self.output_off)
    }
}

#[derive(Clone, Copy)]
struct BgraFromRgb {
    shuffle: [Simd; 2],
    opaque_alpha: Simd,
}

static SHUFFLE0: Simd = setr_epi8([
    2,  1,  0, -1,
    5,  4,  3, -1,
    8,  7,  6, -1,
    11, 10,  9, -1,
]);

static SHUFFLE1: Simd = setr_epi8([
    6,  5,  4, -1,
    9,  8,  7, -1,
    12, 11, 10, -1,
    15, 14, 13, -1,
]);

static OPAQUE_ALPHA: Simd = setr_epu32([
    0xFF_00_00_00,
    0xFF_00_00_00,
    0xFF_00_00_00,
    0xFF_00_00_00,
]);

impl BgraFromRgb {
    #[inline]
    #[target_feature(enable = "sse,sse2,ssse3")]
    fn new() -> Self {
        Self {
            shuffle: [load_const!(SHUFFLE0), load_const!(SHUFFLE1)],
            opaque_alpha: load_const!(OPAQUE_ALPHA),
        }
    }

    #[inline]
    #[target_feature(enable = "sse,sse2,ssse3")]
    fn bgra_from_rgb(self, rgb: [Simd; 3]) -> [Simd; 4] {
        let x0 = alignr_epi8(rgb[1], rgb[0], 8);
        let x1 = alignr_epi8(rgb[2], rgb[1], 8);
        let bgrx = [
            shuffle_epi8(rgb[0], self.shuffle[0]),
            shuffle_epi8(x0, self.shuffle[1]),
            shuffle_epi8(x1, self.shuffle[0]),
            shuffle_epi8(rgb[2], self.shuffle[1]),
        ];
        bgrx.map(|bgrx| or(bgrx, self.opaque_alpha))
    }
}

#[inline]
const fn setr_epi8(elems: [i8; SIMD_SIZE]) -> Simd {
    unsafe { transmute(elems) }
}

#[inline]
const fn setr_epu32(
    elems: [u32; SIMD_SIZE / size_of::<u32>()]
) -> Simd {
    unsafe { transmute(elems) }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn load(simd_ref: &Simd) -> Simd {
    unsafe { _mm_load_si128(simd_ref) }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn load_array<const N: usize>(array_ref: &[Simd; N]) -> [Simd; N] {
    array::from_fn(|i| load(&array_ref[i]))
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
unsafe fn load_last(buf: &[u8]) -> Simd {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    unsafe { load_raw(buf.as_ptr().cast()) }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
unsafe fn load_last_array<const N: usize>(buf: &[u8]) -> [Simd; N] {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < N * size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    unsafe {
        let mut ptr = buf.as_ptr();
        let mut len = buf.len() as isize;
        array::from_fn(|_| {
            if len > 0 {
                let val = load_raw(ptr.cast());
                ptr = ptr.add(SIMD_SIZE);
                len -= SIMD_SIZE as isize;
                val
            } else {
                undefined()
            }
        })
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn store(simd_mut: &mut Simd, val: Simd) {
    unsafe { _mm_store_si128(simd_mut, val) }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn store_array<const N: usize>(simd_mut: &mut [Simd; N], val: [Simd; N]) {
    for i in 0..N {
        store(&mut simd_mut[i], val[i]);
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
pub(super) unsafe fn store_last(mut buf: &mut [u8], mut a: Simd) {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    unsafe {
        if buf.len() >= 8 {
            _mm_storeu_si64(buf.as_mut_ptr(), a);
            a = shuffle_epi32(a, 0b0_01_00_11_10);
            buf = &mut buf[8..];
        }
    }
    if buf.is_empty() {
        return
    }
    super::store_last(buf, cvtsi128_si64(a) as u64)
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
unsafe fn store_last_array<const N: usize>(
    mut buf: &mut [u8],
    simd_array: [Simd; N],
) {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < N * size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    unsafe {
        let mut last = undefined();
        for val in simd_array {
            if buf.len() >= SIMD_SIZE {
                store(&mut *buf.as_mut_ptr().cast(), val);
                buf = &mut buf[SIMD_SIZE..];
            } else {
                last = val;
                break
            }
        }
        if buf.is_empty() {
            return
        }
        store_last(buf, last)
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
unsafe fn load_raw(ptr: *const Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "movdqa {}, xmmword ptr [{}]",
            lateout(xmm_reg) out,
            in(reg) ptr,
            options(pure, readonly, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn packus_epi16(mut a: Simd, b: Simd) -> Simd {
    unsafe {
        asm!(
            "packuswb {}, {}",
            inlateout(xmm_reg) a,
            in(xmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        a
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn mulhi_epu16(mut a: Simd, b: Simd) -> Simd {
    unsafe {
        asm!(
            "pmulhuw {}, {}",
            inlateout(xmm_reg) a,
            in(xmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        a
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn unpackhi_epi8(mut a: Simd, b: Simd) -> Simd {
    unsafe {
        asm!(
            "punpckhbw {}, {}",
            inlateout(xmm_reg) a,
            in(xmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        a
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn unpacklo_epi8(mut a: Simd, b: Simd) -> Simd {
    unsafe {
        asm!(
            "punpcklbw {}, {}",
            inlateout(xmm_reg) a,
            in(xmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        a
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3")]
fn undefined() -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "/* {} */",
            lateout(xmm_reg) out,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}
