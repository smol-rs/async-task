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
use crate::Task;

/// Creates a new task.
///
/// The returned [`Runnable`] is used to poll the `future`, and the [`Task`] is used to await its
/// output.
///
/// Method [`Runnable::run()`] polls the `future` once. Then, the [`Runnable`] vanishes and
/// only reappears when its [`Waker`] wakes the task, thus scheduling it to be run again.
///
/// When the task is woken, its [`Runnable`] is passed to the `schedule` function.
/// The `schedule` function should not attempt to run the [`Runnable`] nor to drop it. Instead, it
/// should push it into a task queue so that it can be processed later.
///
/// If you need to spawn a future that does not implement [`Send`] or isn't `'static`, consider
/// using [`spawn_local()`] or [`spawn_unchecked()`] instead.
///
/// # Examples
///
/// ```
/// // The future inside the task.
/// let future = async {
///     println!("Hello, world!");
/// };
///
/// // A function that schedules the task when it gets woken up.
/// let (s, r) = flume::unbounded();
/// let schedule = move |runnable| s.send(runnable).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (runnable, task) = async_task::spawn(future, schedule);
/// ```
pub fn spawn<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
    S: Fn(Runnable) + Send + Sync + 'static,
{
    unsafe { spawn_unchecked(future, schedule) }
}

/// Creates a new thread-local task.
///
/// This function is same as [`spawn()`], except it does not require [`Send`] on `future`. If the
/// [`Runnable`] is used or dropped on another thread, a panic will occur.
///
/// This function is only available when the `std` feature for this crate is enabled.
///
/// # Examples
///
/// ```
/// use async_task::Runnable;
/// use flume::{Receiver, Sender};
/// use std::rc::Rc;
///
/// thread_local! {
///     // A queue that holds scheduled tasks.
///     static QUEUE: (Sender<Runnable>, Receiver<Runnable>) = flume::unbounded();
/// }
///
/// // Make a non-Send future.
/// let msg: Rc<str> = "Hello, world!".into();
/// let future = async move {
///     println!("{}", msg);
/// };
///
/// // A function that schedules the task when it gets woken up.
/// let s = QUEUE.with(|(s, _)| s.clone());
/// let schedule = move |runnable| s.send(runnable).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (runnable, task) = async_task::spawn_local(future, schedule);
/// ```
#[cfg(feature = "std")]
pub fn spawn_local<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future + 'static,
    F::Output: 'static,
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

    // Wrap the future into one that checks which thread it's on.
    let future = Checked {
        id: thread_id(),
        inner: ManuallyDrop::new(future),
    };

    unsafe { spawn_unchecked(future, schedule) }
}

/// Creates a new task without [`Send`], [`Sync`], and `'static` bounds.
///
/// This function is same as [`spawn()`], except it does not require [`Send`], [`Sync`], and
/// `'static` on `future` and `schedule`.
///
/// Safety requirements:
///
/// - If `future` is not [`Send`], its [`Runnable`] must be used and dropped on the original
///   thread.
/// - If `future` is not `'static`, borrowed variables must outlive its [`Runnable`].
/// - If `schedule` is not [`Send`] and [`Sync`], the task's [`Waker`] must be used and dropped on
///   the original thread.
/// - If `schedule` is not `'static`, borrowed variables must outlive the task's [`Waker`].
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
/// let (runnable, task) = unsafe { async_task::spawn_unchecked(future, schedule) };
/// ```
pub unsafe fn spawn_unchecked<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future,
    S: Fn(Runnable),
{
    // Allocate large futures on the heap.
    let ptr = if mem::size_of::<F>() >= 2048 {
        let future = alloc::boxed::Box::pin(future);
        RawTask::<_, F::Output, S>::allocate(future, schedule)
    } else {
        RawTask::<F, F::Output, S>::allocate(future, schedule)
    };

    let runnable = Runnable { ptr };
    let task = Task {
        ptr,
        _marker: PhantomData,
    };
    (runnable, task)
}

/// A task reference that runs its future.
///
/// At any moment in time, there is at most one [`Runnable`] reference associated with a particular
/// task. Running consumes the [`Runnable`] reference and polls its internal future. If the future
/// is still pending after getting polled, the [`Runnable`] reference simply won't exist until a
/// [`Waker`] notifies the task. If the future completes, its result becomes available to the
/// [`Task`].
///
/// When a task is woken up, its [`Runnable`] reference is recreated and passed to the schedule
/// function. In most executors, scheduling simply pushes the [`Runnable`] reference into a queue
/// of runnable tasks.
///
/// If the [`Runnable`] reference is dropped without getting run, the task is automatically
/// canceled.  When canceled, the task won't be scheduled again even if a [`Waker`] wakes it. It is
/// possible for the [`Task`] to cancel while the [`Runnable`] reference exists, in which
/// case an attempt to run the task won't do anything.
///
/// ----------------
///
/// A runnable future, ready for execution.
///
/// Once a `Runnable` is run, it "vanishes" and only reappears when its future is woken. When it's
/// woken up, its schedule function is called, which means the `Runnable` gets pushed into a task
/// queue in an executor.
pub struct Runnable {
    /// A pointer to the heap-allocated task.
    pub(crate) ptr: NonNull<()>,
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
        let ptr = self.ptr.as_ptr();
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
    /// available to the [`Task`]. And if the future is still pending, the task will have to
    /// be woken up in order to be rescheduled and run again.
    ///
    /// If the task was canceled by a [`Task`] before it gets run, then this method won't do
    /// anything.
    ///
    /// It is possible that polling the future panics, in which case the panic will be propagated
    /// into the caller. It is advised that invocations of this method are wrapped inside
    /// [`catch_unwind`][`std::panic::catch_unwind`]. If a panic occurs, the task is automatically
    /// canceled.
    pub fn run(self) -> bool {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe { ((*header).vtable.run)(ptr) }
    }

    /// Returns a waker associated with this task.
    pub fn waker(&self) -> Waker {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let raw_waker = ((*header).vtable.clone_waker)(ptr);
            Waker::from_raw(raw_waker)
        }
    }
}

impl Drop for Runnable {
    fn drop(&mut self) {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let mut state = (*header).state.load(Ordering::Acquire);

            loop {
                // If the task has been completed or closed, it can't be canceled.
                if state & (COMPLETED | CLOSED) != 0 {
                    break;
                }

                // Mark the task as closed.
                match (*header).state.compare_exchange_weak(
                    state,
                    state | CLOSED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(s) => state = s,
                }
            }

            // Drop the future.
            ((*header).vtable.drop_future)(ptr);

            // Mark the task as unscheduled.
            let state = (*header).state.fetch_and(!SCHEDULED, Ordering::AcqRel);

            // Notify the awaiter that the future has been dropped.
            if state & AWAITER != 0 {
                (*header).notify(None);
            }

            // Drop the task reference.
            ((*header).vtable.drop_ref)(ptr);
        }
    }
}

impl fmt::Debug for Runnable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        f.debug_struct("Runnable")
            .field("header", unsafe { &(*header) })
            .finish()
    }
}
