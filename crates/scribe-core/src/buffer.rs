use std::alloc::{self, Layout};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// Alignment satisfying unbuffered raw-disk I/O on every sector size we accept (512 / 4096).
pub const BUFFER_ALIGN: usize = 4096;

/// Zero-initialised heap buffer aligned to [`BUFFER_ALIGN`].
pub struct AlignedBuffer {
    ptr: NonNull<u8>,
    len: usize,
}

impl AlignedBuffer {
    /// `len` must be a non-zero multiple of [`BUFFER_ALIGN`].
    pub fn zeroed(len: usize) -> Self {
        assert!(
            len > 0 && len.is_multiple_of(BUFFER_ALIGN),
            "buffer length must be a non-zero multiple of {BUFFER_ALIGN}"
        );
        let layout = Self::layout(len);
        // SAFETY: layout has non-zero size.
        let raw = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self { ptr, len }
    }

    fn layout(len: usize) -> Layout {
        Layout::from_size_align(len, BUFFER_ALIGN).expect("valid buffer layout")
    }
}

impl Deref for AlignedBuffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: ptr is a live allocation of len initialised bytes.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl DerefMut for AlignedBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: ptr is a live, uniquely owned allocation of len initialised bytes.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        // SAFETY: allocated in `zeroed` with the same layout.
        unsafe { alloc::dealloc(self.ptr.as_ptr(), Self::layout(self.len)) }
    }
}

// SAFETY: the buffer exclusively owns its allocation.
unsafe impl Send for AlignedBuffer {}
unsafe impl Sync for AlignedBuffer {}
