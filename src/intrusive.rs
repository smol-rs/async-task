use core::cell::UnsafeCell;
use core::fmt;
use core::mem;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use intrusive_collections::linked_list::{LinkedList, LinkedListOps};
use intrusive_collections::{offset_of, Adapter, DefaultLinkOps, LinkOps, PointerOps};

use crate::header::Header;
use crate::runnable::Runnable;

// An atomic version of a LinkedListLink. See https://github.com/Amanieu/intrusive-rs/issues/47 for
// more details.
pub struct AtomicLink {
    prev: UnsafeCell<Option<NonNull<AtomicLink>>>,
    next: UnsafeCell<Option<NonNull<AtomicLink>>>,
    linked: AtomicBool,
}

impl AtomicLink {
    pub fn new() -> AtomicLink {
        AtomicLink {
            prev: UnsafeCell::new(None),
            next: UnsafeCell::new(None),
            linked: AtomicBool::new(false),
        }
    }

    fn is_linked(&self) -> bool {
        self.linked.load(Ordering::Relaxed)
    }
}

impl DefaultLinkOps for AtomicLink {
    type Ops = AtomicLinkOps;

    const NEW: Self::Ops = AtomicLinkOps;
}

impl fmt::Debug for AtomicLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // There's no race-free way to print the values of the `prev` and `next` fields and they
        // likely wouldn't be very useful anyway.
        f.debug_struct("AtomicLink")
            .field("linked", &self.is_linked())
            .finish()
    }
}

// Safe because the only way to mutate `AtomicLink` is via the `LinkedListOps` trait whose methods
// are all unsafe and require that the caller has first called `acquire_link` (and had it return
// true) to use them safely.
unsafe impl Send for AtomicLink {}
unsafe impl Sync for AtomicLink {}

#[derive(Copy, Clone, Debug, Default)]
pub struct AtomicLinkOps;

unsafe impl LinkOps for AtomicLinkOps {
    type LinkPtr = NonNull<AtomicLink>;

    unsafe fn acquire_link(&mut self, ptr: Self::LinkPtr) -> bool {
        !ptr.as_ref().linked.swap(true, Ordering::Acquire)
    }

    unsafe fn release_link(&mut self, ptr: Self::LinkPtr) {
        ptr.as_ref().linked.store(false, Ordering::Release)
    }
}

unsafe impl LinkedListOps for AtomicLinkOps {
    unsafe fn next(&self, ptr: Self::LinkPtr) -> Option<Self::LinkPtr> {
        *ptr.as_ref().next.get()
    }

    unsafe fn prev(&self, ptr: Self::LinkPtr) -> Option<Self::LinkPtr> {
        *ptr.as_ref().prev.get()
    }

    unsafe fn set_next(&mut self, ptr: Self::LinkPtr, next: Option<Self::LinkPtr>) {
        *ptr.as_ref().next.get() = next;
    }

    unsafe fn set_prev(&mut self, ptr: Self::LinkPtr, prev: Option<Self::LinkPtr>) {
        *ptr.as_ref().prev.get() = prev;
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct RunnableOps;
unsafe impl PointerOps for RunnableOps {
    type Value = ();
    type Pointer = Runnable;

    /// # Safety
    /// The raw pointer must have been previously returned by `into_raw`.
    ///
    /// An implementation of `from_raw` must not panic.
    unsafe fn from_raw(&self, value: *const ()) -> Runnable {
        Runnable {
            ptr: NonNull::new_unchecked(value as *mut ()),
        }
    }

    /// Consumes the owned pointer and returns a raw pointer to the owned object.
    fn into_raw(&self, runnable: Runnable) -> *const () {
        let ptr = runnable.ptr.as_ptr();
        mem::forget(runnable);
        ptr
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct RunnableAdapter {
    link_ops: AtomicLinkOps,
    pointer_ops: RunnableOps,
}

impl RunnableAdapter {
    pub fn new() -> RunnableAdapter {
        RunnableAdapter {
            link_ops: AtomicLinkOps,
            pointer_ops: RunnableOps,
        }
    }
}

unsafe impl Adapter for RunnableAdapter {
    type LinkOps = AtomicLinkOps;
    type PointerOps = RunnableOps;

    /// Gets a reference to an object from a reference to a link in that object.
    ///
    /// # Safety
    ///
    /// `link` must be a valid pointer previously returned by `get_link`.
    unsafe fn get_value(&self, link: NonNull<AtomicLink>) -> *const () {
        (link.as_ptr() as *const AtomicLink as *const u8).sub(offset_of!(Header, link)) as *const ()
    }

    /// Gets a reference to the link for the given object.
    ///
    /// # Safety
    ///
    /// `value` must be a valid pointer.
    unsafe fn get_link(&self, value: *const ()) -> NonNull<AtomicLink> {
        let ptr = (value as *const u8).add(offset_of!(Header, link));
        NonNull::new_unchecked(ptr as *mut AtomicLink)
    }

    /// Returns a reference to the link operations.
    fn link_ops(&self) -> &Self::LinkOps {
        &self.link_ops
    }

    /// Returns a reference to the mutable link operations.
    fn link_ops_mut(&mut self) -> &mut Self::LinkOps {
        &mut self.link_ops
    }

    /// Returns a reference to the pointer converter.
    fn pointer_ops(&self) -> &Self::PointerOps {
        &self.pointer_ops
    }
}

/// A [`LinkedList`] of [`Runnable`]s.
///
/// Unlike a traditional linked list, this list embeds the linked list node within its internal
/// structure, removing the need to have a separate container to keep track of [`Runnable`]s.
///
/// # Examples
///
/// ```
/// use std::{panic, thread};
/// use std::sync::{Arc, Condvar, Mutex};
/// use once_cell::sync::Lazy;
/// use async_task::{Runnable, RunnableList};
///
/// // A simple executor that runs tasks on one or more worker threads.
/// #[derive(Default)]
/// struct Executor {
///     queue: Mutex<RunnableList>,
///     cv: Condvar,
/// }
///
/// impl Executor {
///     fn spawn(&self, runnable: Runnable) {
///         self.queue.lock().unwrap().push_back(runnable);
///         self.cv.notify_one();
///     }
///
///     fn next(&self) -> Runnable {
///         let mut queue = self.queue.lock().unwrap();
///         while queue.is_empty() {
///             queue = self.cv.wait(queue).unwrap();
///         }
///         queue.pop_front().unwrap()
///     }
///
///     fn run(&self) {
///         loop {
///             let runnable = self.next();
///             let _ignore_panic = panic::catch_unwind(|| runnable.run());
///         }
///     }
/// }
///
/// static EXECUTOR: Lazy<Arc<Executor>> = Lazy::new(|| {
///     let ex = Arc::new(Executor::default());
///     let ex2 = Arc::clone(&ex);
///     thread::spawn(move || ex2.run());
///     ex
/// });
///
/// let schedule = |runnable| EXECUTOR.spawn(runnable);
///
/// // Create a task with a simple future and the schedule function.
/// let (runnable, task) = async_task::spawn(async { 7 + 11 }, schedule);
///
/// // Schedule the task and await its output.
/// runnable.schedule();
/// assert_eq!(smol::future::block_on(task), 18);
/// ```
pub type RunnableList = LinkedList<RunnableAdapter>;
