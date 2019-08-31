use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem;
use std::ptr::NonNull;

use crate::header::Header;
use crate::raw::RawTask;
use crate::JoinHandle;

/// Creates a new task.
///
/// This constructor returns a [`Task`] reference that runs the future and a [`JoinHandle`] that
/// awaits its result.
///
/// When run, the task polls `future`. When woken, it gets scheduled for running by the `schedule`
/// function. Argument `tag` is an arbitrary piece of data stored inside the task.
///
/// [`Task`]: struct.Task.html
/// [`JoinHandle`]: struct.JoinHandle.html
///
/// # Examples
///
/// ```
/// use crossbeam::channel;
///
/// // The future inside the task.
/// let future = async {
///     println!("Hello, world!");
/// };
///
/// // If the task gets woken, it will be sent into this channel.
/// let (s, r) = channel::unbounded();
/// let schedule = move |task| s.send(task).unwrap();
///
/// // Create a task with the future and the schedule function.
/// let (task, handle) = async_task::spawn(future, schedule, ());
/// ```
pub fn spawn<F, R, S, T>(future: F, schedule: S, tag: T) -> (Task<T>, JoinHandle<R, T>)
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
    S: Fn(Task<T>) + Send + Sync + 'static,
    T: Send + Sync + 'static,
{
    let raw_task = RawTask::<F, R, S, T>::allocate(tag, future, schedule);
    let task = Task {
        raw_task,
        _marker: PhantomData,
    };
    let handle = JoinHandle {
        raw_task,
        _marker: PhantomData,
    };
    (task, handle)
}

/// A task reference that runs its future.
///
/// The [`Task`] reference "owns" the task itself and is able to run it. Running consumes the
/// [`Task`] reference and polls its internal future. If the future is still pending after getting
/// polled, the [`Task`] reference simply won't exist until a [`Waker`] notifies the task. If the
/// future completes, its result becomes available to the [`JoinHandle`].
///
/// When the task is woken, the [`Task`] reference is recreated and passed to the schedule
/// function. In most executors, scheduling simply pushes the [`Task`] reference into a queue of
/// runnable tasks.
///
/// If the [`Task`] reference is dropped without being run, the task is cancelled. When cancelled,
/// the task won't be scheduled again even if a [`Waker`] wakes it. It is possible for the
/// [`JoinHandle`] to cancel while the [`Task`] reference exists, in which case an attempt to run
/// the task won't do anything.
///
/// [`run()`]: struct.Task.html#method.run
/// [`JoinHandle`]: struct.JoinHandle.html
/// [`Task`]: struct.Task.html
/// [`Waker`]: https://doc.rust-lang.org/std/task/struct.Waker.html
pub struct Task<T> {
    /// A pointer to the heap-allocated task.
    pub(crate) raw_task: NonNull<()>,

    /// A marker capturing the generic type `T`.
    pub(crate) _marker: PhantomData<T>,
}

unsafe impl<T> Send for Task<T> {}
unsafe impl<T> Sync for Task<T> {}

impl<T> Task<T> {
    /// Schedules the task.
    ///
    /// This is a convenience method that simply reschedules the task by passing it to its schedule
    /// function.
    ///
    /// If the task is cancelled, this method won't do anything.
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
    /// This method polls the task's future. If the future completes, its result will become
    /// available to the [`JoinHandle`]. And if the future is still pending, the task will have to
    /// be woken in order to be rescheduled and then run again.
    ///
    /// If the task was cancelled by a [`JoinHandle`] before it gets run, then this method won't do
    /// anything.
    ///
    /// It is possible that polling the future panics, in which case the panic will be propagated
    /// into the caller. It is advised that invocations of this method are wrapped inside
    /// [`catch_unwind`].
    ///
    /// If a panic occurs, the task is automatically cancelled.
    ///
    /// [`JoinHandle`]: struct.JoinHandle.html
    /// [`catch_unwind`]: https://doc.rust-lang.org/std/panic/fn.catch_unwind.html
    pub fn run(self) {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe {
            ((*header).vtable.run)(ptr);
        }
    }

    /// Cancels the task.
    ///
    /// When cancelled, the task won't be scheduled again even if a [`Waker`] wakes it. An attempt
    /// to run it won't do anything.
    ///
    /// [`Waker`]: https://doc.rust-lang.org/std/task/struct.Waker.html
    pub fn cancel(&self) {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            (*header).cancel();
        }
    }

    /// Returns a reference to the tag stored inside the task.
    pub fn tag(&self) -> &T {
        let offset = Header::offset_tag::<T>();
        let ptr = self.raw_task.as_ptr();

        unsafe {
            let raw = (ptr as *mut u8).add(offset) as *const T;
            &*raw
        }
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            // Cancel the task.
            (*header).cancel();

            // Drop the future.
            ((*header).vtable.drop_future)(ptr);

            // Drop the task reference.
            ((*header).vtable.decrement)(ptr);
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Task<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        f.debug_struct("Task")
            .field("header", unsafe { &(*header) })
            .field("tag", self.tag())
            .finish()
    }
}
