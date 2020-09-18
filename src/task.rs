use core::fmt;
use core::future::Future;
use core::marker::{PhantomData, Unpin};
use core::mem;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::{Context, Poll};

use crate::header::Header;
use crate::state::*;

/// A handle that awaits the result of a task.
///
/// This type is a future that resolves to an `Option<T>` where:
///
/// * `None` indicates the task has panicked or was canceled.
/// * `Some(result)` indicates the task has completed with `result` of type `T`.
#[must_use = "tasks get canceled when dropped, use `.detach()` to run them in the background"]
pub struct Task<T> {
    /// A raw task pointer.
    pub(crate) raw_task: NonNull<()>,

    /// A marker capturing generic type `T`.
    pub(crate) _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Task<T> {}
unsafe impl<T> Sync for Task<T> {}

impl<T> Unpin for Task<T> {}

#[cfg(feature = "std")]
impl<T> std::panic::UnwindSafe for Task<T> {}
#[cfg(feature = "std")]
impl<T> std::panic::RefUnwindSafe for Task<T> {}

impl<T> Task<T> {
    pub fn detach(self) {
        let mut this = self;
        let _out = this.set_detached();
        mem::forget(this);
    }

    pub async fn cancel(self) -> Option<T> {
        let mut this = self;
        this.set_canceled();

        struct Fut<T>(Task<T>);

        impl<T> Future for Fut<T> {
            type Output = Option<T>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                self.0.poll_result(cx)
            }
        }

        Fut(this).await
    }

    fn set_canceled(&mut self) {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let mut state = (*header).state.load(Ordering::Acquire);

            loop {
                // If the task has been completed or closed, it can't be canceled.
                if state & (COMPLETED | CLOSED) != 0 {
                    break;
                }

                // If the task is not scheduled nor running, we'll need to schedule it.
                let new = if state & (SCHEDULED | RUNNING) == 0 {
                    (state | SCHEDULED | CLOSED) + REFERENCE
                } else {
                    state | CLOSED
                };

                // Mark the task as closed.
                match (*header).state.compare_exchange_weak(
                    state,
                    new,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        // If the task is not scheduled nor running, schedule it one more time so
                        // that its future gets dropped by the executor.
                        if state & (SCHEDULED | RUNNING) == 0 {
                            ((*header).vtable.schedule)(ptr);
                        }

                        // Notify the awaiter that the task has been closed.
                        if state & AWAITER != 0 {
                            (*header).notify(None);
                        }

                        break;
                    }
                    Err(s) => state = s,
                }
            }
        }
    }

    fn set_detached(&mut self) -> Option<T> {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            // A place where the output will be stored in case it needs to be dropped.
            let mut output = None;

            // Optimistically assume the `Task` is being detached just after creating the
            // task. This is a common case so if the handle is not used, the overhead of it is only
            // one compare-exchange operation.
            if let Err(mut state) = (*header).state.compare_exchange_weak(
                SCHEDULED | HANDLE | REFERENCE,
                SCHEDULED | REFERENCE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                loop {
                    // If the task has been completed but not yet closed, that means its output
                    // must be dropped.
                    if state & COMPLETED != 0 && state & CLOSED == 0 {
                        // Mark the task as closed in order to grab its output.
                        match (*header).state.compare_exchange_weak(
                            state,
                            state | CLOSED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(_) => {
                                // Read the output.
                                output =
                                    Some((((*header).vtable.get_output)(ptr) as *mut T).read());

                                // Update the state variable because we're continuing the loop.
                                state |= CLOSED;
                            }
                            Err(s) => state = s,
                        }
                    } else {
                        // If this is the last reference to the task and it's not closed, then
                        // close it and schedule one more time so that its future gets dropped by
                        // the executor.
                        let new = if state & (!(REFERENCE - 1) | CLOSED) == 0 {
                            SCHEDULED | CLOSED | REFERENCE
                        } else {
                            state & !HANDLE
                        };

                        // Unset the handle flag.
                        match (*header).state.compare_exchange_weak(
                            state,
                            new,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(_) => {
                                // If this is the last reference to the task, we need to either
                                // schedule dropping its future or destroy it.
                                if state & !(REFERENCE - 1) == 0 {
                                    if state & CLOSED == 0 {
                                        ((*header).vtable.schedule)(ptr);
                                    } else {
                                        ((*header).vtable.destroy)(ptr);
                                    }
                                }

                                break;
                            }
                            Err(s) => state = s,
                        }
                    }
                }
            }

            output
        }
    }

    fn poll_result(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let mut state = (*header).state.load(Ordering::Acquire);

            loop {
                // If the task has been closed, notify the awaiter and return `None`.
                if state & CLOSED != 0 {
                    // If the task is scheduled or running, we need to wait until its future is
                    // dropped.
                    if state & (SCHEDULED | RUNNING) != 0 {
                        // Replace the waker with one associated with the current task.
                        (*header).register(cx.waker());

                        // Reload the state after registering. It is possible changes occurred just
                        // before registration so we need to check for that.
                        state = (*header).state.load(Ordering::Acquire);

                        // If the task is still scheduled or running, we need to wait because its
                        // future is not dropped yet.
                        if state & (SCHEDULED | RUNNING) != 0 {
                            return Poll::Pending;
                        }
                    }

                    // Even though the awaiter is most likely the current task, it could also be
                    // another task.
                    (*header).notify(Some(cx.waker()));
                    return Poll::Ready(None);
                }

                // If the task is not completed, register the current task.
                if state & COMPLETED == 0 {
                    // Replace the waker with one associated with the current task.
                    (*header).register(cx.waker());

                    // Reload the state after registering. It is possible that the task became
                    // completed or closed just before registration so we need to check for that.
                    state = (*header).state.load(Ordering::Acquire);

                    // If the task has been closed, restart.
                    if state & CLOSED != 0 {
                        continue;
                    }

                    // If the task is still not completed, we're blocked on it.
                    if state & COMPLETED == 0 {
                        return Poll::Pending;
                    }
                }

                // Since the task is now completed, mark it as closed in order to grab its output.
                match (*header).state.compare_exchange(
                    state,
                    state | CLOSED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        // Notify the awaiter. Even though the awaiter is most likely the current
                        // task, it could also be another task.
                        if state & AWAITER != 0 {
                            (*header).notify(Some(cx.waker()));
                        }

                        // Take the output from the task.
                        let output = ((*header).vtable.get_output)(ptr) as *mut T;
                        return Poll::Ready(Some(output.read()));
                    }
                    Err(s) => state = s,
                }
            }
        }
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.set_canceled();
        self.set_detached();
    }
}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.poll_result(cx) {
            Poll::Ready(t) => Poll::Ready(t.expect("task has failed")),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> fmt::Debug for Task<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr = self.raw_task.as_ptr();
        let header = ptr as *const Header;

        f.debug_struct("Task")
            .field("header", unsafe { &(*header) })
            .finish()
    }
}
