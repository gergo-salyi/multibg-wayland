use super::*;

pub fn memchr(haystack: &[u8], needle: u8) -> Option<usize> {
    let haystack_ptr = haystack.as_ptr();
    let haystack_len = haystack.len();
    if haystack_len == 0 {
        return None
    }
    unsafe {
        let ptr = libc::memchr(
            haystack_ptr.cast(),
            needle as c_int,
            haystack_len,
        ).cast::<u8>();
        if ptr.is_null() {
            None
        } else {
            let idx = ptr.offset_from_unsigned(haystack_ptr);
            assert_unchecked(idx < haystack_len);
            Some(idx)
        }
    }
}

pub fn memmem(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let haystack_ptr = haystack.as_ptr();
    let haystack_len = haystack.len();
    let needle_ptr = needle.as_ptr();
    let needle_len = needle.len();
    if haystack_len < needle_len {
        return None
    }
    unsafe {
        let ptr = libc::memmem(
            haystack_ptr.cast(),
            haystack_len,
            needle_ptr.cast(),
            needle_len,
        ).cast::<u8>();
        if ptr.is_null() {
            None
        } else {
            let idx = ptr.offset_from_unsigned(haystack_ptr);
            assert_unchecked(idx.unchecked_add(needle_len) <= haystack_len);
            Some(idx)
        }
    }
}
