use super::*;
use std::arch::x86_64::{
    __m256i as Simd,
    _mm256_add_epi8 as add_epi8,
    _mm256_blend_epi32 as blend_epi32,
    _mm256_castsi256_si128 as cast_si128,
    _mm256_extracti128_si256,
    _mm256_load_si256,
    _mm256_min_epu8 as min_epu8,
    _mm256_or_si256 as or,
    _mm256_permute4x64_epi64 as permute4x64,
    _mm256_set1_epi8 as set1_epi8,
    _mm256_set1_epi16 as set1_epi16,
    _mm256_shuffle_epi8 as shuffle_epi8,
    _mm256_store_si256,
    _mm256_subs_epu8 as subs_epu8,
    _mm256_xor_si256 as xor,
    __m128i,
    _mm_store_si128,
};

const SIMD_SIZE: usize = size_of::<Simd>();

macro_rules! load_const {
    ($sym:ident) => {
        unsafe {
            let _ = const { let _type_check: Simd = $sym; };
            let out: Simd;
            asm!(
                "vmovdqa {}, ymmword ptr [rip+{}]",
                lateout(ymm_reg) out,
                sym $sym,
                options(pure, readonly, preserves_flags, nostack),
            );
            out
        }
    }
}

macro_rules! broadcast_const_u8 {
    ($sym:ident) => {
        unsafe {
            let _ = const { let _type_check: u8 = $sym; };
            let out: Simd;
            asm!(
                "vpbroadcastb {}, byte ptr [rip+{}]",
                lateout(ymm_reg) out,
                sym $sym,
                options(pure, readonly, preserves_flags, nostack),
            );
            out
        }
    }
}

macro_rules! broadcast_const_u32 {
    ($sym:ident) => {
        unsafe {
            let _ = const { let _type_check: u32 = $sym; };
            let out: Simd;
            asm!(
                "vpbroadcastd {}, dword ptr [rip+{}]",
                lateout(ymm_reg) out,
                sym $sym,
                options(pure, readonly, preserves_flags, nostack),
            );
            out
        }
    }
}

#[target_feature(enable = "avx,avx2")]
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

#[target_feature(enable = "avx,avx2")]
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

#[target_feature(enable = "avx,avx2")]
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

static BIAS: u8 = 128u8;

impl Levels {
    #[inline]
    #[target_feature(enable = "avx,avx2")]
    fn from_color_tranform(transform: &ColorTransform) -> Self {
        Self {
            input_min: set1_epi8(transform.input_min as i8),
            input_rel_max: set1_epi8(transform.input_rel_max as i8),
            bias: broadcast_const_u8!(BIAS),
            factor: set1_epi16(transform.factor as i16),
            xor_term: set1_epi8(transform.xor_term as i8),
            output_off: set1_epi8(transform.output_off as i8),
        }
    }

    #[inline]
    #[target_feature(enable = "avx,avx2")]
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
    shuffle: Simd,
    opaque_alpha: Simd,
}

static SHUFFLE: Simd = setr_epi8([
    2,  1,  0, -1,
    5,  4,  3, -1,
    8,  7,  6, -1,
    11, 10,  9, -1,
    6,  5,  4, -1,
    9,  8,  7, -1,
    12, 11, 10, -1,
    15, 14, 13, -1,
]);

static OPAQUE_ALPHA: u32 = 0xFF_00_00_00;

impl BgraFromRgb {
    #[inline]
    #[target_feature(enable = "avx,avx2")]
    fn new() -> Self {
        Self {
            shuffle: load_const!(SHUFFLE),
            opaque_alpha: broadcast_const_u32!(OPAQUE_ALPHA),
        }
    }

    // rgb[0]   0  1  2  3
    // rgb[1]   4  5  6  7
    // rgb[2]   8  9 10 11
    //
    // tmp1     4  5  2  3
    // tmp2     8  9  6  7
    //
    // bgra[0]  0  1  1  2
    // bgra[1]  3  4  4  5
    // bgra[2]  6  7  7  8
    // bgra[3]  9 10 10 11
    #[inline]
    #[target_feature(enable = "avx,avx2")]
    fn bgra_from_rgb(self, rgb: [Simd; 3]) -> [Simd; 4] {
        let tmp1 = blend_epi32(rgb[0], rgb[1], 0b_0000_1111);
        let tmp2 = blend_epi32(rgb[1], rgb[2], 0b_0000_1111);
        let bgra = [
            permute4x64(rgb[0], 0b_10_01_01_00),
            permute4x64(tmp1, 0b_01_00_00_11),
            permute4x64(tmp2, 0b_00_11_11_10),
            permute4x64(rgb[2], 0b_11_10_10_01),
        ];
        bgra.map(|bgra| shuffle_epi8(bgra, self.shuffle))
            .map(|bgra| or(bgra, self.opaque_alpha))
    }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
fn extract_hi(simd: Simd) -> __m128i {
    _mm256_extracti128_si256(simd, 1)
}

#[inline]
const fn setr_epi8(elems: [i8; SIMD_SIZE]) -> Simd {
    unsafe { transmute(elems) }
}

// #[inline]
// #[target_feature(enable = "avx,avx2")]
// fn setones() -> Simd {
//     let tmp = undefined();
//     cmpeq_epi32(tmp, tmp)
// }

#[inline]
#[target_feature(enable = "avx,avx2")]
fn load(simd_ref: &Simd) -> Simd {
    unsafe { _mm256_load_si256(simd_ref) }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
fn load_array<const N: usize>(array_ref: &[Simd; N]) -> [Simd; N] {
    array::from_fn(|i| load(&array_ref[i]))
}

#[inline]
#[target_feature(enable = "avx,avx2")]
unsafe fn load_last(buf: &[u8]) -> Simd {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    // Aligned loads can never fault
    unsafe {
        let ptr = buf.as_ptr();
        let out: Simd;
        asm!(
            "vmovdqa {}, ymmword ptr [{}]",
            lateout(ymm_reg) out,
            in(reg) ptr,
            options(pure, readonly, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
unsafe fn load_last_array<const N: usize>(mut buf: &[u8]) -> [Simd; N] {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < N * size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    unsafe {
        array::from_fn(|_| {
            let ptr = buf.as_ptr();
            let len = buf.len();
            if len >= SIMD_SIZE {
                let val = load(&*ptr.cast());
                buf = &buf[SIMD_SIZE..];
                val
            } else if len != 0 {
                let val = load_last(buf);
                buf = &buf[len..];
                val
            } else {
                undefined()
            }
        })
    }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
fn store(simd_mut: &mut Simd, val: Simd) {
    unsafe { _mm256_store_si256(simd_mut, val) }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
fn store_array<const N: usize>(simd_mut: &mut [Simd; N], val: [Simd; N]) {
    for i in 0..N {
        store(&mut simd_mut[i], val[i]);
    }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
unsafe fn store_last(mut buf: &mut [u8], a: Simd) {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    unsafe {
        let a = if buf.len() >= 16 {
            _mm_store_si128(
                buf.as_mut_ptr() as _,
                cast_si128(a)
            );
            buf = &mut buf[16..];
            extract_hi(a)
        } else {
            cast_si128(a)
        };
        if buf.is_empty() {
            return
        }
        super::ssse3::store_last(buf, a)
    }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
unsafe fn store_last_array<const N: usize>(
    mut buf: &mut [u8],
    simd_array: [Simd; N],
) {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < N * size_of::<Simd>());
    debug_assert!(buf.as_ptr().cast::<Simd>().is_aligned());
    unsafe {
        for val in simd_array {
            let ptr = buf.as_mut_ptr();
            let len = buf.len();
            if len >= SIMD_SIZE {
                store(&mut *ptr.cast(), val);
                buf = &mut buf[SIMD_SIZE..];
            } else if len != 0 {
                store_last(buf, val);
                buf = &mut buf[len..];
            }
        }
    }
}

#[inline]
#[target_feature(enable = "avx2")]
fn packus_epi16(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpackuswb {}, {}, {}",
            lateout(ymm_reg) out,
            in(ymm_reg) a,
            in(ymm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx2")]
fn mulhi_epu16(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpmulhuw {}, {}, {}",
            lateout(ymm_reg) out,
            in(ymm_reg) a,
            in(ymm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx2")]
fn unpackhi_epi8(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpunpckhbw {}, {}, {}",
            lateout(ymm_reg) out,
            in(ymm_reg) a,
            in(ymm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx2")]
fn unpacklo_epi8(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpunpcklbw {}, {}, {}",
            lateout(ymm_reg) out,
            in(ymm_reg) a,
            in(ymm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx,avx2")]
fn undefined() -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "/* {} */",
            lateout(ymm_reg) out,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}
