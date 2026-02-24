pub(crate) use self::inner::{allocate, deallocate, Allocator, Global};

// Nightly `allocator_api` feature case.
#[cfg(feature = "allocator_api")]
mod inner {
    pub use crate::alloc::alloc::{Allocator, Global};

    use crate::alloc::alloc::Layout;
    use core::ptr::NonNull;

    pub(crate) fn allocate<A: Allocator>(alloc: &A, layout: Layout) -> Result<NonNull<u8>, ()> {
        match alloc.allocate(layout) {
            Ok(ptr) => Ok(ptr.cast()),
            Err(_) => Err(()),
        }
    }

    /// # Safety
    ///
    /// - `ptr` must have been allocated by `alloc` using the same allocator.
    /// - `layout` must be the same layout that was used to allocate `ptr`.
    /// - `ptr` must not be used after this call.
    pub(crate) unsafe fn deallocate<A: Allocator>(alloc: &A, ptr: *mut u8, layout: Layout) {
        alloc.deallocate(NonNull::new_unchecked(ptr), layout)
    }
}

// Non-nightly `allocator-api2` feature case.
#[cfg(all(feature = "allocator-api2", not(feature = "allocator_api")))]
mod inner {
    pub use allocator_api2::alloc::{Allocator, Global};

    use crate::alloc::alloc::Layout;
    use core::ptr::NonNull;

    pub(crate) fn allocate<A: Allocator>(alloc: &A, layout: Layout) -> Result<NonNull<u8>, ()> {
        match alloc.allocate(layout) {
            Ok(ptr) => Ok(ptr.cast()),
            Err(_) => Err(()),
        }
    }

    /// # Safety
    ///
    /// - `ptr` must have been allocated by `alloc` using the same allocator.
    /// - `layout` must be the same layout that was used to allocate `ptr`.
    /// - `ptr` must not be used after this call.
    pub(crate) unsafe fn deallocate<A: Allocator>(alloc: &A, ptr: *mut u8, layout: Layout) {
        alloc.deallocate(NonNull::new_unchecked(ptr), layout)
    }
}

// Default case without allocator api.
#[cfg(not(any(feature = "allocator_api", feature = "allocator-api2")))]
mod inner {
    use crate::alloc::alloc::{alloc, dealloc, Layout};
    use core::ptr::NonNull;

    /// # Safety
    ///
    /// Used only within the crate and implemented only for [`Global`].
    pub unsafe trait Allocator {
        fn allocate(&self, layout: Layout) -> Result<NonNull<u8>, ()>;

        /// # Safety
        ///
        /// - `ptr` must have been returned by a previous call to `allocate` on this allocator.
        /// - `layout` must be the same layout used in that `allocate` call.
        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout);
    }

    #[derive(Copy, Clone)]
    pub struct Global;

    impl Default for Global {
        #[inline]
        fn default() -> Self {
            Global
        }
    }

    unsafe impl Allocator for Global {
        #[inline]
        fn allocate(&self, layout: Layout) -> Result<NonNull<u8>, ()> {
            unsafe { NonNull::new(alloc(layout)).ok_or(()) }
        }

        #[inline]
        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
            dealloc(ptr.as_ptr(), layout);
        }
    }

    pub(crate) fn allocate<A: Allocator>(alloc: &A, layout: Layout) -> Result<NonNull<u8>, ()> {
        alloc.allocate(layout)
    }

    /// # Safety
    ///
    /// - `ptr` must have been allocated by `alloc` using the same allocator.
    /// - `layout` must be the same layout that was used to allocate `ptr`.
    /// - `ptr` must not be used after this call.
    pub(crate) unsafe fn deallocate<A: Allocator>(alloc: &A, ptr: *mut u8, layout: Layout) {
        alloc.deallocate(NonNull::new_unchecked(ptr), layout)
    }
}
