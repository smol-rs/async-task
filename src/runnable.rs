use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::mem;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::Waker;

use crate::header::Header;
use crate::raw::RawTask;
use crate::state::*;
use crate::JoinHandle;

/// Creates a new task.
///
/// This constructor returns a [`Runnable`] reference that runs the future and a [`JoinHandle`]
/// that awaits its result.
///
/// When run, the task polls `future`. When woken up, it gets scheduled for running by the
/// `schedule` function.
///
/// The schedule function should not attempt to run the task nor to drop it. Instead, it should
/// push the task into some kind of queue so that it can be processed later.
///
/// If you need to spawn a future that does not implement [`Send`], consider using the
/// [`spawn_local`] function instead.
///
/// # Examples
///
/// ```
/// // The future inside the task.
/// let future = async {
///     println!("Hello, world!");
/// };
///
/// // If the task gets woken up, it will be sent into this channel.
/// let (s, r) = flume::unbounded();
/// let schedule = move |runnable| s.send(runnable).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (runnable, handle) = async_task::spawn(future, schedule);
/// ```
pub fn spawn<F, T, S>(future: F, schedule: S) -> (Runnable, JoinHandle<T>)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
    S: Fn(Runnable) + Send + Sync + 'static,
{
    // Allocate large futures on the heap.
    let raw_task = if mem::size_of::<F>() >= 2048 {
        let future = alloc::boxed::Box::pin(future);
        RawTask::<_, T, S>::allocate(future, schedule)
    } else {
        RawTask::<F, T, S>::allocate(future, schedule)
    };

    let task = Runnable { raw_task };
    let handle = JoinHandle {
        raw_task,
        _marker: PhantomData,
    };
    (task, handle)
}

/// Creates a new local task.
///
/// This constructor returns a [`Runnable`] reference that runs the future and a [`JoinHandle`]
/// that awaits its result.
///
/// When run, the task polls `future`. When woken up, it gets scheduled for running by the
/// `schedule` function.
///
/// The schedule function should not attempt to run the task nor to drop it. Instead, it should
/// push the task into some kind of queue so that it can be processed later.
///
/// Unlike [`spawn`], this function does not require the future to implement [`Send`]. If the
/// [`Runnable`] reference is run or dropped on a thread it was not created on, a panic will occur.
///
/// **NOTE:** This function is only available when the `std` feature for this crate is enabled (it
/// is by default).
///
/// # Examples
///
/// ```
/// // The future inside the task.
/// let future = async {
///     println!("Hello, world!");
/// };
///
/// // If the task gets woken up, it will be sent into this channel.
/// let (s, r) = flume::unbounded();
/// let schedule = move |runnable| s.send(runnable).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (runnable, handle) = async_task::spawn_local(future, schedule);
/// ```
#[cfg(feature = "std")]
pub fn spawn_local<F, T, S>(future: F, schedule: S) -> (Runnable, JoinHandle<T>)
where
    F: Future<Output = T> + 'static,
    T: 'static,
    S: Fn(Runnable) + Send + Sync + 'static,
{
    use std::mem::ManuallyDrop;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::thread::{self, ThreadId};

    #[inline]
    fn thread_id() -> ThreadId {
        thread_local! {
            static ID: ThreadId = thread::current().id();
        }
        ID.try_with(|id| *id)
            .unwrap_or_else(|_| thread::current().id())
    }

    struct Checked<F> {
        id: ThreadId,
        inner: ManuallyDrop<F>,
    }

    impl<F> Drop for Checked<F> {
        fn drop(&mut self) {
            assert!(
                self.id == thread_id(),
                "local task dropped by a thread that didn't spawn it"
            );
            unsafe {
                ManuallyDrop::drop(&mut self.inner);
            }
        }
    }

    impl<F: Future> Future for Checked<F> {
        type Output = F::Output;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            assert!(
                self.id == thread_id(),
                "local task polled by a thread that didn't spawn it"
            );
            unsafe { self.map_unchecked_mut(|c| &mut *c.inner).poll(cx) }
        }
    }

    // Wrap the future into one that which thread it's on.
    let future = Checked {
        id: thread_id(),
        inner: ManuallyDrop::new(future),
    };

    // Allocate large futures on the heap.
    let raw_task = if mem::size_of::<F>() >= 2048 {
        let future = alloc::boxed::Box::pin(future);
        RawTask::<_, T, S>::allocate(future, schedule)
    } else {
        RawTask::<_, T, S>::allocate(future, schedule)
    };

    let task = Runnable { raw_task };
    let handle = JoinHandle {
        raw_task,
        _marker: PhantomData,
    };
    (task, handle)
}

/// A task reference that runs its future.
///
/// At any moment in time, there is at most one [`Runnable`] reference associated with a particular
/// task. Running consumes the [`Runnable`] reference and polls its internal future. If the future
/// is still pending after getting polled, the [`Runnable`] reference simply won't exist until a
/// [`Waker`] notifies the task. If the future completes, its result becomes available to the
/// [`JoinHandle`].
///
/// When a task is woken up, its [`Runnable`] reference is recreated and passed to the schedule
/// function. In most executors, scheduling simply pushes the [`Runnable`] reference into a queue
/// of runnable tasks.
///
/// If the [`Runnable`] reference is dropped without getting run, the task is automatically
/// canceled.  When canceled, the task won't be scheduled again even if a [`Waker`] wakes it. It is
/// possible for the [`JoinHandle`] to cancel while the [`Runnable`] reference exists, in which
/// case an attempt to run the task won't do anything.
pub struct Runnable {
    /// A pointer to the heap-allocated task.
    pub(crate) raw_task: NonNull<()>,
}

unsafe impl Send for Runnable {}
unsafe impl Sync for Runnable {}

#[cfg(feature = "std")]
impl std::panic::UnwindSafe for Runnable {}
#[cfg(feature = "std")]
impl std::panic::RefUnwindSafe for Runnable {}

impl Runnable {
    /// Schedules the task.
    ///
    /// This is a convenience method that simply reschedules the task by passing it to its schedule
    /// function.
    ///
    /// If the task is canceled, this method won't do anything.
    pub fn schedule(self) {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe {
            ((*header).vtable.schedule)(ptr);
        }
    }

    /// Runs the task.
    ///
    /// Returns `true` if the task was woken while running, in which case it gets rescheduled at
    /// the end of this method invocation.
    ///
    /// This method polls the task's future. If the future completes, its result will become
    /// available to the [`JoinHandle`]. And if the future is still pending, the task will have to
    /// be woken up in order to be rescheduled and run again.
    ///
    /// If the task was canceled by a [`JoinHandle`] before it gets run, then this method won't do
    /// anything.
    ///
    /// It is possible that polling the future panics, in which case the panic will be propagated
    /// into the caller. It is advised that invocations of this method are wrapped inside
    /// [`catch_unwind`]. If a panic occurs, the task is automatically canceled.
    pub fn run(self) -> bool {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe { ((*header).vtable.run)(ptr) }
    }

    /// Returns a waker associated with this task.
    pub fn waker(&self) -> Waker {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let raw_waker = ((*header).vtable.clone_waker)(ptr);
            Waker::from_raw(raw_waker)
        }
    }
}

impl Drop for Runnable {
    fn drop(&mut self) {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            // Cancel the task.
            (*header).cancel();

            // Drop the future.
            ((*header).vtable.drop_future)(ptr);

            // Mark the task as unscheduled.
            let state = (*header).state.fetch_and(!SCHEDULED, Ordering::AcqRel);

            // Notify the awaiter that the future has been dropped.
            if state & AWAITER != 0 {
                (*header).notify(None);
            }

            // Drop the task reference.
            ((*header).vtable.drop_task)(ptr);
        }
    }
}

impl fmt::Debug for Runnable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        f.debug_struct("Runnable")
            .field("header", unsafe { &(*header) })
            .finish()
    }
}
