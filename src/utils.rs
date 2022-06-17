use core::alloc::Layout;
use core::mem;

/// Aborts the process.
///
/// To abort, this function simply panics while panicking.
pub(crate) fn abort() -> ! {
    struct Panic;

    impl Drop for Panic {
        fn drop(&mut self) {
            panic!("aborting the process");
        }
    }

    let _panic = Panic;
    panic!("aborting the process");
}

/// Calls a function and aborts if it panics.
///
/// This is useful in unsafe code where we can't recover from panics.
#[inline]
pub(crate) fn abort_on_panic<T>(f: impl FnOnce() -> T) -> T {
    struct Bomb;

    impl Drop for Bomb {
        fn drop(&mut self) {
            abort();
        }
    }

    let bomb = Bomb;
    let t = f();
    mem::forget(bomb);
    t
}

/// Returns the layout for `a` followed by `b` and the offset of `b`.
///
/// This function was adapted from the currently unstable `Layout::extend()`:
/// https://doc.rust-lang.org/nightly/std/alloc/struct.Layout.html#method.extend
#[inline]
pub(crate) const fn extend(a: Layout, b: Layout) -> (Layout, usize) {
    let new_align = const_max(a.align(), b.align());
    let pad = padding_needed_for(a, b.align());

    // Cannot use unwrap here due to it not being const.
    let offset = match a.size().checked_add(pad) {
        Some(offset) => offset,
        None => panic!(),
    };
    let new_size = match offset.checked_add(b.size()) {
        Some(new_size) => new_size,
        None => panic!(),
    };

    let layout = match Layout::from_size_align(new_size, new_align) {
        Ok(layout) => layout,
        Err(_) => panic!(),
    };
    (layout, offset)
}

/// Returns the padding after `layout` that aligns the following address to `align`.
///
/// This function was adapted from the currently unstable `Layout::padding_needed_for()`:
/// https://doc.rust-lang.org/nightly/std/alloc/struct.Layout.html#method.padding_needed_for
#[inline]
pub(crate) const fn padding_needed_for(layout: Layout, align: usize) -> usize {
    let len = layout.size();
    let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
    len_rounded_up.wrapping_sub(len)
}

/// A const-friendly max function, since trait functions cannot be const.
#[inline]
pub(crate) const fn const_max(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}
