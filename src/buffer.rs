use std::{
    alloc::{alloc, dealloc, handle_alloc_error, Layout, realloc},
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    slice,
};

// A reusable buffer with large enough alignment for any SIMD use
pub struct Buf(NonNull<[u8]>);

impl Buf {
    pub const ALIGN: usize = 64;

    pub fn new() -> Self {
        Self(NonNull::slice_from_raw_parts(
            NonNull::new(ptr::without_provenance_mut(Self::ALIGN)).unwrap(),
            0,
        ))
    }

    pub fn reserve(&mut self, new_size: usize) {
        let old_size = self.len();
        if new_size <= old_size {
            return
        }
        let new_layout = Self::layout_for_size(new_size);
        unsafe {
            let new_ptr = if self.is_empty() {
                alloc(new_layout)
            } else {
                realloc(self.as_mut_ptr(), self.layout(), new_size)
            };
            let Some(new_non_null) = NonNull::new(new_ptr) else {
                handle_alloc_error(new_layout)
            };
            new_non_null.add(old_size).write_bytes(0, new_size - old_size);
            self.0 = NonNull::slice_from_raw_parts(new_non_null, new_size);
        }
    }

    pub unsafe fn view<T>(&self) -> &[T] {
        unsafe {
            slice::from_raw_parts(
                self.as_ptr().cast::<T>(),
                self.len() / size_of::<T>(),
            )
        }
    }

    // pub unsafe fn view_mut<T>(&mut self) -> &mut [T] {
    //     unsafe {
    //         slice::from_raw_parts_mut(
    //             self.as_mut_ptr().cast::<T>(),
    //             self.len() / size_of::<T>(),
    //         )
    //     }
    // }

    pub fn view_u16(&self) -> &[u16] {
        unsafe { self.view::<u16>() }
    }

    // pub fn view_u16_mut(&mut self) -> &mut [u16] {
    //     unsafe { self.view_mut::<u16>() }
    // }

    pub fn view_f32(&self) -> &[f32] {
        unsafe { self.view::<f32>() }
    }

    // pub fn view_f32_mut(&mut self) -> &mut [f32] {
    //     unsafe { self.view_mut::<f32>() }
    // }

    fn layout(&self) -> Layout {
        Self::layout_for_size(self.len())
    }

    fn layout_for_size(size: usize) -> Layout {
        Layout::from_size_align(size, Self::ALIGN).unwrap()
    }
}

impl Deref for Buf {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}

impl DerefMut for Buf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut() }
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        if !self.is_empty() {
            unsafe { dealloc(self.as_mut_ptr(), self.layout()) }
        }
    }
}
