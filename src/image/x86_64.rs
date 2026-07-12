//
// The codegen for SIMD intrinsics is @#$%& HORRIBLE !!!
//
// The Rust std started rewriting std::arch::x86_64 internally
// on the top of the still experimental and broken portable Rust SIMD,
// replacing the use of the LLVM backend x86 intrinsics, as a result:
// - _mm_mulhi_epu16 is broken in Rust since 1.75
// - _mm_packus_epi16 just broke in Rust 1.96
// In addition there are bugs in LLVM, like:
// - both Rust and Clang puts some garbage shuffle between
//   _mm256_unpack{lo,hi}_epi8 and _mm256_mulhi_epu16
// - _mm_unpack{lo,hi}_epi8 was broken in Rust 1.73-1.94 and in Clang 17-21
// Only GCC can reliably generate optimal code for the C equivalents.
// Also:
// - Rust _mm_load_si128 follows Rust ptr dereference semantics
//   and not movdqa semantics so cannot read past the end of a slice
// - setting a local var with _mm_set1_epi8 makes it to be reloaded
//   from .ro for every block where it is used even without reg pressure
//
// Writing the problematic parts with inline asm for now.

mod ssse3;
mod avx2;
mod avx512bw;

use super::*;
use std::{
    arch::asm,
    array,
    mem::transmute,
    slice,
};

// const PAGE_SIZE: usize = 4096;

pub fn select_fns() -> Functions {
    #[cfg(debug_assertions)]
    let (allow_avx512, allow_avx, allow_sse) = (
        std::env::var_os("MULTIBG_DEBUG_NOAVX512").is_none(),
        std::env::var_os("MULTIBG_DEBUG_NOAVX").is_none(),
        std::env::var_os("MULTIBG_DEBUG_NOSSE").is_none(),
    );
    #[cfg(not(debug_assertions))]
    let (allow_avx512, allow_avx, allow_sse) = (true, true, true);
    if allow_avx512
        && is_x86_feature_detected!("avx512f")
        && is_x86_feature_detected!("avx512bw")
    {
        debug!("Using AVX-512 image processing functions");
        Functions {
            levels_in_place_fn: avx512bw::levels_in_place,
            levels_to_bgra_fn: avx512bw::levels_to_bgra,
            bgra_from_rgb_fn: avx512bw::bgra_from_rgb,
        }
    } else if allow_avx
        && is_x86_feature_detected!("avx")
        && is_x86_feature_detected!("avx2")
    {
        debug!("Using AVX2 image processing functions");
        Functions {
            levels_in_place_fn: avx2::levels_in_place,
            levels_to_bgra_fn: avx2::levels_to_bgra,
            bgra_from_rgb_fn: avx2::bgra_from_rgb,
        }
    } else if allow_sse
        && is_x86_feature_detected!("sse")
        && is_x86_feature_detected!("sse2")
        && is_x86_feature_detected!("ssse3")
    {
        debug!("Using SSSE3 image processing functions");
        Functions {
            levels_in_place_fn: ssse3::levels_in_place,
            levels_to_bgra_fn: ssse3::levels_to_bgra,
            bgra_from_rgb_fn: ssse3::bgra_from_rgb,
        }
    } else {
        Functions {
            levels_in_place_fn: levels_in_place,
            levels_to_bgra_fn: levels_to_bgra,
            bgra_from_rgb_fn: bgra_from_rgb,
        }
    }
}

#[inline]
unsafe fn split_aligned<T>(buf: &[u8]) -> (&[T], &[u8]) {
    let ptr = buf.as_ptr();
    let total_size = buf.len();
    let count = total_size / size_of::<T>();
    let suffix_offset = count * size_of::<T>();
    let suffix_size = total_size % size_of::<T>();
    unsafe { (
        slice::from_raw_parts(ptr as _, count),
        slice::from_raw_parts(ptr.add(suffix_offset), suffix_size),
    ) }
}

#[inline]
unsafe fn split_aligned_mut<T>(buf: &mut [u8]) -> (&mut [T], &mut [u8]) {
    let ptr = buf.as_mut_ptr();
    let total_size = buf.len();
    let count = total_size / size_of::<T>();
    let suffix_offset = count * size_of::<T>();
    let suffix_size = total_size % size_of::<T>();
    unsafe { (
        slice::from_raw_parts_mut(ptr as _, count),
        slice::from_raw_parts_mut(ptr.add(suffix_offset), suffix_size),
    ) }
}

#[inline(never)]
fn store_last(buf: &mut [u8], r: u64) {
    debug_assert!(!buf.is_empty());
    debug_assert!(buf.len() < 8);
    let len = buf.len();
    let r = r.to_ne_bytes();
    match len {
        1 => buf.copy_from_slice(&r[..len]),
        2 => buf.copy_from_slice(&r[..len]),
        3 => buf.copy_from_slice(&r[..len]),
        4 => buf.copy_from_slice(&r[..len]),
        5 => buf.copy_from_slice(&r[..len]),
        6 => buf.copy_from_slice(&r[..len]),
        7 => buf.copy_from_slice(&r[..len]),
        _ => (),
    }
}

/*
#[inline]
fn access_within_page::<T>(ptr: *const u8) -> bool {
    match PAGE_SIZE.checked_sub(size_of::<T>()) {
        Some(max_offset) => ptr.addr() % PAGE_SIZE <= max_offset,
        None => false
    }
}

#[inline]
#[target_feature(enable = "sse,sse2,ssse3,sse4.1")]
fn bgra_from_rgb_sse41(
    rgb: [__m128i; 3],
    shuffle: [__m128i; 4],
    opaque: __m128i,
) -> [__m128i; 4] {
    let mut bgra0 = _mm_shuffle_epi8(rgb[0], rgb[0]);
    let mut bgra1 = _mm_shuffle_epi8(
        _mm_blend_epi16::<0x0F>(rgb[1], rgb[0]),
        shuffle[1],
    );
    let mut bgra2 = _mm_shuffle_epi8(
        _mm_blend_epi16::<0x0F>(rgb[2], rgb[1]),
        shuffle[2],
    );
    let mut bgra3 = _mm_shuffle_epi8(rgb[2], rgb[3]);
    bgra0 = _mm_or_si128(bgra0, opaque);
    bgra1 = _mm_or_si128(bgra1, opaque);
    bgra2 = _mm_or_si128(bgra2, opaque);
    bgra3 = _mm_or_si128(bgra3, opaque);
    [bgra0, bgra1, bgra2, bgra3]
}
*/


/*
unsafe fn cast_safe_levels_fn(
    f: unsafe fn(&ColorTransform, &mut [u8]),
) -> fn(&ColorTransform, &mut [u8]) {
    transmute(f)
}
*/
/*
use std::arch::x86_64::*;

pub struct ColorTransform {
    input_min: u8,
    input_rel_max: u8,
    factor: u16,
    xor_term: u8,
    output_off: u8,
}

#[no_mangle]
#[inline(never)]
#[target_feature(enable = "sse,sse2")]
pub unsafe fn levels_sse2(transform: &ColorTransform, buf: &mut [__m128i]) {
    unsafe {
        let input_min = _mm_set1_epi8(transform.input_min as i8);
        let input_rel_max = _mm_set1_epi8(transform.input_rel_max as i8);
        let bias = _mm_set1_epi8(128u8 as i8);
        let factor = _mm_set1_epi16(transform.factor as i16);
        let xor_term = _mm_set1_epi8(transform.xor_term as i8);
        let output_off = _mm_set1_epi8(transform.output_off as i8);
        for vector_mut in buf {
            let input = *vector_mut;
            let half_clamped = _mm_subs_epu8(input, input_min);
            let clamped = _mm_min_epu8(half_clamped, input_rel_max);
            let unpacked_lo = _mm_unpacklo_epi8(bias, clamped);
            let unpacked_hi = _mm_unpackhi_epi8(bias, clamped);
            let scaled_lo =_mm_mulhi_epu16(unpacked_lo, factor);
            let scaled_hi = _mm_mulhi_epu16(unpacked_hi, factor);
            let packed = _mm_packus_epi16(scaled_lo, scaled_hi);
            let maybe_inverted = _mm_xor_si128(packed, xor_term);
            let output = _mm_add_epi8(maybe_inverted, output_off);
            *vector_mut = output;
        }
    }
}

#[no_mangle]
#[inline(never)]
#[target_feature(enable = "avx,avx2")]
pub unsafe fn levels_avx2(transform: &ColorTransform, buf: &mut [__m256i]) {
    let input_min = _mm256_set1_epi8(transform.input_min as i8);
    let input_rel_max = _mm256_set1_epi8(transform.input_rel_max as i8);
    let bias = _mm256_set1_epi8(128u8 as i8);
    let factor = _mm256_set1_epi16(transform.factor as i16);
    let xor_term = _mm256_set1_epi8(transform.xor_term as i8);
    let output_off = _mm256_set1_epi8(transform.output_off as i8);
    for vector_mut in buf {
        let input = *vector_mut;
        let half_clamped = _mm256_subs_epu8(input, input_min);
        let clamped = _mm256_min_epu8(half_clamped, input_rel_max);
        let unpacked_lo = _mm256_unpacklo_epi8(bias, clamped);
        let unpacked_hi = _mm256_unpackhi_epi8(bias, clamped);
        let scaled_lo =_mm256_mulhi_epu16(unpacked_lo, factor);
        let scaled_hi = _mm256_mulhi_epu16(unpacked_hi, factor);
        let packed = _mm256_packus_epi16(scaled_lo, scaled_hi);
        let maybe_inverted = _mm256_xor_si256(packed, xor_term);
        let output = _mm256_add_epi8(maybe_inverted, output_off);
        *vector_mut = output;
    }
}
*/
/*
#include <stddef.h>
#include <stdint.h>
#include <emmintrin.h>
#include <immintrin.h>

struct ColorTransform {
    uint8_t  input_min;
    uint8_t  input_rel_max;
    uint16_t factor;
    uint8_t  xor_term;
    uint8_t  output_off;
};

__attribute__((target("sse,sse2")))
static inline __m128i all_ones() {
    __m128i tmp = _mm_setzero_si128();
    return _mm_cmpeq_epi32(tmp, tmp);
}

__attribute__((target("sse,sse2")))
static inline __m128i lo64_ones() {
    return _mm_move_epi64(all_ones());
}

__attribute__((target("sse,sse2")))
static inline __m128i bgra_opaque_from_bgrx(__m128i bgrx) {
    return _mm_or_si128(
        bgrx,
        _mm_slli_epi32(all_ones(), 24)
    );
}

__attribute__((target("sse,sse2")))
void levels_sse2(
    const struct ColorTransform *transform,
    __m128i *buf,
    size_t len
) {
    __m128i input_min = _mm_set1_epi8((char)transform->input_min);
    __m128i input_rel_max = _mm_set1_epi8((char)transform->input_rel_max);
    __m128i bias = _mm_set1_epi8((char)128);
    __m128i factor = _mm_set1_epi16((short)transform->factor);
    __m128i xor_term = _mm_set1_epi8((char)transform->xor_term);
    __m128i output_off = _mm_set1_epi8((char)transform->output_off);

    for (size_t i = 0; i < len; ++i) {
        __m128i input = buf[i];
        __m128i half_clamped = _mm_subs_epu8(input, input_min);
        __m128i clamped = _mm_min_epu8(half_clamped, input_rel_max);
        __m128i unpacked_lo = _mm_unpacklo_epi8(bias, clamped);
        __m128i unpacked_hi = _mm_unpackhi_epi8(bias, clamped);
        __m128i scaled_lo = _mm_mulhi_epu16(unpacked_lo, factor);
        __m128i scaled_hi = _mm_mulhi_epu16(unpacked_hi, factor);
        __m128i packed = _mm_packus_epi16(scaled_lo, scaled_hi);
        __m128i maybe_inverted = _mm_xor_si128(packed, xor_term);
        __m128i output = _mm_add_epi8(maybe_inverted, output_off);
        buf[i] = output;
    }
}

__attribute__((target("avx,avx2")))
void levels_avx2(
    const struct ColorTransform *transform,
    __m256i *buf,
    size_t len
) {
    __m256i input_min = _mm256_set1_epi8((char)transform->input_min);
    __m256i input_rel_max = _mm256_set1_epi8((char)transform->input_rel_max);
    __m256i bias = _mm256_set1_epi8((char)128);
    __m256i factor = _mm256_set1_epi16((short)transform->factor);
    __m256i xor_term = _mm256_set1_epi8((char)transform->xor_term);
    __m256i output_off = _mm256_set1_epi8((char)transform->output_off);

    for (size_t i = 0; i < len; ++i) {
        __m256i input = buf[i];
        __m256i half_clamped = _mm256_subs_epu8(input, input_min);
        __m256i clamped = _mm256_min_epu8(half_clamped, input_rel_max);
        __m256i unpacked_lo = _mm256_unpacklo_epi8(bias, clamped);
        __m256i unpacked_hi = _mm256_unpackhi_epi8(bias, clamped);
        __m256i scaled_lo = _mm256_mulhi_epu16(unpacked_lo, factor);
        __m256i scaled_hi = _mm256_mulhi_epu16(unpacked_hi, factor);
        __m256i packed = _mm256_packus_epi16(scaled_lo, scaled_hi);
        __m256i maybe_inverted = _mm256_xor_si256(packed, xor_term);
        __m256i output = _mm256_add_epi8(maybe_inverted, output_off);
        buf[i] = output;
    }
}
*/
