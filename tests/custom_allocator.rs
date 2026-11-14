#![cfg_attr(feature = "allocator_api", feature(allocator_api))]
#![cfg(any(feature = "allocator_api", feature = "allocator-api2"))]

use std::future::Future;
use std::panic::catch_unwind;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use async_task::Runnable;
use easy_parallel::Parallel;

#[cfg(feature = "allocator_api")]
extern crate alloc;
#[cfg(feature = "allocator_api")]
pub use alloc::alloc::{AllocError, Allocator, Global};

#[cfg(all(feature = "allocator-api2", not(feature = "allocator_api")))]
pub use allocator_api2::alloc::{AllocError, Allocator, Global};

// Creates a future with event counters.
//
// Usage: `future!(f, POLL, DROP)`
//
// The future `f` always returns `Poll::Ready`.
// When it gets polled, `POLL` is incremented.
// When it gets dropped, `DROP` is incremented.
macro_rules! future {
    ($name:pat, $poll:ident, $drop:ident) => {
        static $poll: AtomicUsize = AtomicUsize::new(0);
        static $drop: AtomicUsize = AtomicUsize::new(0);

        let $name = {
            struct Fut(#[allow(dead_code)] Box<i32>);

            impl Future for Fut {
                type Output = Box<i32>;

                fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                    $poll.fetch_add(1, Ordering::SeqCst);
                    Poll::Ready(Box::new(42))
                }
            }

            impl Drop for Fut {
                fn drop(&mut self) {
                    $drop.fetch_add(1, Ordering::SeqCst);
                }
            }

            Fut(Box::new(0))
        };
    };
}

// Creates a future with event counters.
//
// Usage: `panic_future!(f, POLL, DROP)`
//
// The future `f` sleeps for 400 ms and then panics.
// When it gets polled, `POLL` is incremented.
// When it gets dropped, `DROP` is incremented.
macro_rules! panic_future {
    ($name:pat, $poll:ident, $drop:ident) => {
        static $poll: AtomicUsize = AtomicUsize::new(0);
        static $drop: AtomicUsize = AtomicUsize::new(0);

        let $name = {
            struct Fut(#[allow(dead_code)] Box<i32>);

            impl Future for Fut {
                type Output = ();

                fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                    $poll.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(400));
                    panic!()
                }
            }

            impl Drop for Fut {
                fn drop(&mut self) {
                    $drop.fetch_add(1, Ordering::SeqCst);
                }
            }

            Fut(Box::new(0))
        };
    };
}

// Creates a schedule function with event counters.
//
// Usage: `schedule!(s, SCHED, DROP)`
//
// The schedule function `s` does nothing.
// When it gets invoked, `SCHED` is incremented.
// When it gets dropped, `DROP` is incremented.
macro_rules! schedule {
    ($name:pat, $sched:ident, $drop:ident) => {
        static $drop: AtomicUsize = AtomicUsize::new(0);
        static $sched: AtomicUsize = AtomicUsize::new(0);

        let $name = {
            struct Guard(#[allow(dead_code)] Box<i32>);

            impl Drop for Guard {
                fn drop(&mut self) {
                    $drop.fetch_add(1, Ordering::SeqCst);
                }
            }

            let guard = Guard(Box::new(0));
            move |_runnable: Runnable| {
                let _ = &guard;
                $sched.fetch_add(1, Ordering::SeqCst);
            }
        };
    };
}

// Creates an allocator with event counters.
//
// Usage: `allocator!(a, ALLOC, DEALLOC, DROP)`
//
// The allocator `a` wraps `Global` and tracks allocations.
// When `allocate` is called, `ALLOC` is incremented.
// When `deallocate` is called, `DEALLOC` is incremented.
// When it gets dropped, `DROP` is incremented.
macro_rules! allocator {
    ($name:pat, $alloc:ident, $dealloc:ident, $drop:ident) => {
        static $alloc: AtomicUsize = AtomicUsize::new(0);
        static $dealloc: AtomicUsize = AtomicUsize::new(0);
        static $drop: AtomicUsize = AtomicUsize::new(0);

        let $name = {
            struct TrackedAllocator(#[allow(dead_code)] Box<i32>);

            unsafe impl Send for TrackedAllocator {}
            unsafe impl Sync for TrackedAllocator {}

            impl Drop for TrackedAllocator {
                fn drop(&mut self) {
                    $drop.fetch_add(1, Ordering::SeqCst);
                }
            }

            unsafe impl Allocator for TrackedAllocator {
                fn allocate(
                    &self,
                    layout: std::alloc::Layout,
                ) -> Result<std::ptr::NonNull<[u8]>, AllocError> {
                    $alloc.fetch_add(1, Ordering::SeqCst);
                    Global.allocate(layout)
                }

                unsafe fn deallocate(
                    &self,
                    ptr: std::ptr::NonNull<u8>,
                    layout: std::alloc::Layout,
                ) {
                    $dealloc.fetch_add(1, Ordering::SeqCst);
                    Global.deallocate(ptr, layout)
                }
            }

            TrackedAllocator(Box::new(0))
        };
    };
}

#[test]
fn detach_and_drop() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    allocator!(a, ALLOC, DEALLOC, DROP_A);
    let (runnable, task) = async_task::spawn_in(f, s, a);

    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    task.detach();
    assert_eq!(POLL.load(Ordering::SeqCst), 0);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    drop(runnable);
    assert_eq!(POLL.load(Ordering::SeqCst), 0);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 1);
}

#[test]
fn cancel_and_run() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    allocator!(a, ALLOC, DEALLOC, DROP_A);
    let (runnable, task) = async_task::spawn_in(f, s, a);

    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    drop(task);
    assert_eq!(POLL.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    runnable.run();
    assert_eq!(POLL.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 1);
}

#[test]
fn run_and_cancel() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    allocator!(a, ALLOC, DEALLOC, DROP_A);
    let (runnable, task) = async_task::spawn_in(f, s, a);

    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    assert_eq!(POLL.load(Ordering::SeqCst), 0);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    runnable.run();
    assert_eq!(POLL.load(Ordering::SeqCst), 1);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    drop(task);
    assert_eq!(POLL.load(Ordering::SeqCst), 1);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 1);
}

#[test]
fn cancel_during_run() {
    panic_future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    allocator!(a, ALLOC, DEALLOC, DROP_A);
    let (runnable, task) = async_task::spawn_in(f, s, a);

    assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_A.load(Ordering::SeqCst), 0);

    Parallel::new()
        .add(|| {
            assert!(catch_unwind(|| runnable.run()).is_err());
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
            assert_eq!(DEALLOC.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_A.load(Ordering::SeqCst), 1);
        })
        .add(|| {
            thread::sleep(Duration::from_millis(200));

            drop(task);
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(ALLOC.load(Ordering::SeqCst), 1);
            assert_eq!(DEALLOC.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_A.load(Ordering::SeqCst), 0);
        })
        .run();
}
