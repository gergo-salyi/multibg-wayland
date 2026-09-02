#![allow(clippy::identity_op)]

use super::*;
use std::arch::x86_64::{
    // AVX512F, AVX512BW
    __m512i as Simd,
    _mm512_add_epi8 as add_epi8,
    _mm512_load_si512,
    _mm512_mask_storeu_epi8,
    _mm512_min_epu8 as min_epu8,
    _mm512_or_si512 as or,
    _mm512_permutexvar_epi64 as permutexvar_epi64,
    _mm512_permutex2var_epi64 as permutex2var_epi64,
    _mm512_set1_epi8 as set1_epi8,
    _mm512_set1_epi16 as set1_epi16,
    _mm512_shuffle_epi8 as shuffle_epi8,
    _mm512_store_si512,
    _mm512_subs_epu8 as subs_epu8,
    _mm512_xor_si512 as xor,
    // AVX512VBMI
    _mm512_permutexvar_epi8 as permutexvar_epi8,
    _mm512_permutex2var_epi8 as permutex2var_epi8,
};

const SIMD_SIZE: usize = size_of::<Simd>();

macro_rules! load_const {
    ($sym:ident) => {
        unsafe {
            let _ = const { let _type_check: Simd = $sym; };
            let out: Simd;
            asm!(
                "vmovdqa64 {}, zmmword ptr [rip+{}]",
                lateout(zmm_reg) out,
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
                lateout(zmm_reg) out,
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
                lateout(zmm_reg) out,
                sym $sym,
                options(pure, readonly, preserves_flags, nostack),
            );
            out
        }
    }
}

macro_rules! cvt_const_epu8_epi64 {
    ($sym:ident) => {
        unsafe {
            let _ = const { let _type_check: u64 = $sym; };
            let out: Simd;
            asm!(
                "vpmovzxbq {}, qword ptr [rip+{}]",
                lateout(zmm_reg) out,
                sym $sym,
                options(pure, readonly, preserves_flags, nostack),
            );
            out
        }
    }
}

#[target_feature(enable = "avx512f,avx512bw")]
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

#[target_feature(enable = "avx512f,avx512bw")]
pub unsafe fn levels_to_bgra_bw(
    transform: &ColorTransform,
    dst: &mut [u8],
    src: &[u8],
) {
    let levels = Levels::from_color_tranform(transform);
    let colors = BgraFromRgbBw::new();
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

#[target_feature(enable = "avx512f,avx512bw,avx512vbmi")]
pub unsafe fn levels_to_bgra_vbmi(
    transform: &ColorTransform,
    dst: &mut [u8],
    src: &[u8],
) {
    let levels = Levels::from_color_tranform(transform);
    let colors = BgraFromRgbVbmi::new();
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

#[target_feature(enable = "avx512f,avx512bw")]
pub unsafe fn bgra_from_rgb_bw(dst: &mut [u8], src: &[u8]) {
    let colors = BgraFromRgbBw::new();
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

#[target_feature(enable = "avx512f,avx512bw,avx512vbmi")]
pub unsafe fn bgra_from_rgb_vbmi(dst: &mut [u8], src: &[u8]) {
    let colors = BgraFromRgbVbmi::new();
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

static BIAS: u8 = 128;

impl Levels {
    #[inline]
    #[target_feature(enable = "avx512f,avx512bw")]
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
    #[target_feature(enable = "avx512f,avx512bw")]
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

static OPAQUE_ALPHA: u32 = 0xFF_00_00_00;

#[derive(Clone, Copy)]
struct BgraFromRgbBw {
    permute: [Simd; 4],
    shuffle: Simd,
    opaque_alpha: Simd,
}

impl BgraFromRgbBw {
    #[inline]
    #[target_feature(enable = "avx512f,avx512bw")]
    fn new() -> Self {
        Self {
            permute: [
                cvt_const_epu8_epi64!(BW_PERMUTE0),
                cvt_const_epu8_epi64!(BW_PERMUTE1),
                cvt_const_epu8_epi64!(BW_PERMUTE2),
                cvt_const_epu8_epi64!(BW_PERMUTE3),
            ],
            shuffle: load_const!(BW_SHUFFLE),
            opaque_alpha: broadcast_const_u32!(OPAQUE_ALPHA),
        }
    }

    // rgb[0]   0  1  2  3  4  5  6  7
    // rgb[1]   8  9 10 11 12 13 14 15
    // rgb[2]  16 17 18 19 20 21 22 23
    //
    // bgra[0]  0  1  1  2  3  4  4  5
    // bgra[1]  6  7  7  8  9 10 10 11
    // bgra[2] 12 13 13 14 15 16 16 17
    // bgra[3] 18 19 19 20 21 22 22 23
    #[inline]
    #[target_feature(enable = "avx512f,avx512bw")]
    fn bgra_from_rgb(self, rgb: [Simd; 3]) -> [Simd; 4] {
        [
            permutexvar_epi64(self.permute[0], rgb[0]),
            permutex2var_epi64(rgb[0], self.permute[1], rgb[1]),
            permutex2var_epi64(rgb[1], self.permute[2], rgb[2]),
            permutexvar_epi64(self.permute[3], rgb[2]),
        ]
            .map(|bgra| shuffle_epi8(bgra, self.shuffle))
            .map(|bgra| or(bgra, self.opaque_alpha))
    }
}

static BW_PERMUTE0: u64 = u64::from_ne_bytes([0, 1, 1, 2, 3, 4, 4, 5]);
static BW_PERMUTE1: u64 = u64::from_ne_bytes([6, 7, 7, 8|0, 8|1, 8|2, 8|2, 8|3]);
static BW_PERMUTE2: u64 = u64::from_ne_bytes([4, 5, 5, 6, 7, 8|0, 8|0, 8|1]);
static BW_PERMUTE3: u64 = u64::from_ne_bytes([2, 3, 3, 4, 5, 6, 6, 7]);

static BW_SHUFFLE: Simd = setr_epi8([
     2,  1,  0, -1,
     5,  4,  3, -1,
     8,  7,  6, -1,
    11, 10,  9, -1,
     6,  5,  4, -1,
     9,  8,  7, -1,
    12, 11, 10, -1,
    15, 14, 13, -1,
     2,  1,  0, -1,
     5,  4,  3, -1,
     8,  7,  6, -1,
    11, 10,  9, -1,
     6,  5,  4, -1,
     9,  8,  7, -1,
    12, 11, 10, -1,
    15, 14, 13, -1,
]);

#[derive(Clone, Copy)]
struct BgraFromRgbVbmi {
    shuffle: [Simd; 4],
    opaque_alpha: Simd,
}

impl BgraFromRgbVbmi {
    #[inline]
    #[target_feature(enable = "avx512f,avx512bw,avx512vbmi")]
    fn new() -> Self {
        Self {
            shuffle: [
                load_const!(VBMI_SHUFFLE0),
                load_const!(VBMI_SHUFFLE1),
                load_const!(VBMI_SHUFFLE2),
                load_const!(VBMI_SHUFFLE3),
            ],
            opaque_alpha: broadcast_const_u32!(OPAQUE_ALPHA),
        }
    }

    #[inline]
    #[target_feature(enable = "avx512f,avx512bw,avx512vbmi")]
    fn bgra_from_rgb(self, rgb: [Simd; 3]) -> [Simd; 4] {
        [
            permutexvar_epi8(self.shuffle[0], rgb[0]),
            permutex2var_epi8(rgb[0], self.shuffle[1], rgb[1]),
            permutex2var_epi8(rgb[1], self.shuffle[2], rgb[2]),
            permutexvar_epi8(self.shuffle[3], rgb[2]),
        ]
            .map(|bgra| or(bgra, self.opaque_alpha))
    }
}

static VBMI_SHUFFLE0: Simd = setr_epi8([
     2,  1,  0, -1,
     5,  4,  3, -1,
     8,  7,  6, -1,
    11, 10,  9, -1,
    14, 13, 12, -1,
    17, 16, 15, -1,
    20, 19, 18, -1,
    23, 22, 21, -1,

    26, 25, 24, -1,
    29, 28, 27, -1,
    32, 31, 30, -1,
    35, 34, 33, -1,
    38, 37, 36, -1,
    41, 40, 39, -1,
    44, 43, 42, -1,
    47, 46, 45, -1,
]);

static VBMI_SHUFFLE1: Simd = setr_epi8([
       50,    49,    48, -1,
       53,    52,    51, -1,
       56,    55,    54, -1,
       59,    58,    57, -1,
       62,    61,    60, -1,
    64| 1, 64| 0,    63, -1,
    64| 4, 64| 3, 64| 2, -1,
    64| 7, 64| 6, 64| 5, -1,
    64|10, 64| 9, 64| 8, -1,
    64|13, 64|12, 64|11, -1,
    64|16, 64|15, 64|14, -1,
    64|19, 64|18, 64|17, -1,
    64|22, 64|21, 64|20, -1,
    64|25, 64|24, 64|23, -1,
    64|28, 64|27, 64|26, -1,
    64|31, 64|30, 64|29, -1,
]);

static VBMI_SHUFFLE2: Simd = setr_epi8([
       34,    33,    32, -1,
       37,    36,    35, -1,
       40,    39,    38, -1,
       43,    42,    41, -1,
       46,    45,    44, -1,
       49,    48,    47, -1,
       52,    51,    50, -1,
       55,    54,    53, -1,
       58,    57,    56, -1,
       61,    60,    59, -1,
    64| 0,    63,    62, -1,
    64| 3, 64| 2, 64| 1, -1,
    64| 6, 64| 5, 64| 4, -1,
    64| 9, 64| 8, 64| 7, -1,
    64|12, 64|11, 64|10, -1,
    64|15, 64|14, 64|13, -1,
]);

static VBMI_SHUFFLE3: Simd = setr_epi8([
    18, 17, 16, -1,
    21, 20, 19, -1,
    24, 23, 22, -1,
    27, 26, 25, -1,
    30, 29, 28, -1,
    33, 32, 31, -1,
    36, 35, 34, -1,
    39, 38, 37, -1,
    42, 41, 40, -1,
    45, 44, 43, -1,
    48, 47, 46, -1,
    51, 50, 49, -1,
    54, 53, 52, -1,
    57, 56, 55, -1,
    60, 59, 58, -1,
    63, 62, 61, -1,
]);

#[inline]
const fn setr_epi8(elems: [i8; SIMD_SIZE]) -> Simd {
    unsafe { transmute(elems) }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn load(simd_ref: &Simd) -> Simd {
    unsafe { _mm512_load_si512(simd_ref) }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn load_array<const N: usize>(array_ref: &[Simd; N]) -> [Simd; N] {
    array::from_fn(|i| load(&array_ref[i]))
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn load_last(buf: &[u8]) -> Simd {
    debug_assert_valid_last_access::<Simd>(buf);
    unsafe { load_raw(buf.as_ptr().cast()) }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn load_last_array<const N: usize>(buf: &[u8]) -> [Simd; N] {
    debug_assert_valid_last_access::<[Simd; N]>(buf);
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
#[target_feature(enable = "avx512f,avx512bw")]
fn store(simd_mut: &mut Simd, val: Simd) {
    unsafe { _mm512_store_si512(simd_mut, val) }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn store_array<const N: usize>(simd_mut: &mut [Simd; N], val: [Simd; N]) {
    for i in 0..N {
        store(&mut simd_mut[i], val[i]);
    }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn store_last(buf: &mut [u8], a: Simd) {
    debug_assert_valid_last_access::<Simd>(buf);
    unsafe {
        _mm512_mask_storeu_epi8(
            buf.as_mut_ptr() as _,
            !0 >> (64 - buf.len()),
            a,
        );
    }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn store_last_array<const N: usize>(
    mut buf: &mut [u8],
    simd_array: [Simd; N],
) {
    debug_assert_valid_last_access::<[Simd; N]>(buf);
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
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn load_raw(ptr: *const Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vmovdqa64 {}, zmmword ptr [{}]",
            lateout(zmm_reg) out,
            in(reg) ptr,
            options(pure, readonly, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn packus_epi16(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpackuswb {}, {}, {}",
            lateout(zmm_reg) out,
            in(zmm_reg) a,
            in(zmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn mulhi_epu16(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpmulhuw {}, {}, {}",
            lateout(zmm_reg) out,
            in(zmm_reg) a,
            in(zmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn unpackhi_epi8(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpunpckhbw {}, {}, {}",
            lateout(zmm_reg) out,
            in(zmm_reg) a,
            in(zmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn unpacklo_epi8(a: Simd, b: Simd) -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "vpunpcklbw {}, {}, {}",
            lateout(zmm_reg) out,
            in(zmm_reg) a,
            in(zmm_reg) b,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}

#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
fn undefined() -> Simd {
    unsafe {
        let out: Simd;
        asm!(
            "/* {} */",
            lateout(zmm_reg) out,
            options(pure, nomem, preserves_flags, nostack),
        );
        out
    }
}
