use core::cell::UnsafeCell;
use core::fmt;
use core::task::{RawWaker, RawWakerVTable, Waker};

#[cfg(not(feature = "portable-atomic"))]
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering;
#[cfg(feature = "portable-atomic")]
use portable_atomic::AtomicUsize;

use crate::raw::TaskVTable;
use crate::state::*;
use crate::utils::abort;
use crate::utils::abort_on_panic;
use crate::ScheduleInfo;

pub(crate) enum Action {
    Schedule,
    Destroy,
}

pub(crate) struct Header {
    /// Current state of the task.
    ///
    /// Contains flags representing the current state and the reference count.
    pub(crate) state: AtomicUsize,

    /// The task that is blocked on the `Task` handle.
    ///
    /// This waker needs to be woken up once the task completes or is closed.
    pub(crate) awaiter: UnsafeCell<Option<Waker>>,

    /// The virtual table.
    ///
    /// In addition to the actual waker virtual table, it also contains pointers to several other
    /// methods necessary for bookkeeping the heap-allocated task.
    pub(crate) vtable: &'static TaskVTable,

    /// Whether or not a panic that occurs in the task should be propagated.
    #[cfg(feature = "std")]
    pub(crate) propagate_panic: bool,
}

impl Header {
    /// Notifies the awaiter blocked on this task.
    ///
    /// If the awaiter is the same as the current waker, it will not be notified.
    #[inline]
    pub(crate) fn notify(&self, current: Option<&Waker>) {
        if let Some(w) = self.take(current) {
            abort_on_panic(|| w.wake());
        }
    }

    /// Takes the awaiter blocked on this task.
    ///
    /// If there is no awaiter or if it is the same as the current waker, returns `None`.
    #[inline]
    pub(crate) fn take(&self, current: Option<&Waker>) -> Option<Waker> {
        // Set the bit indicating that the task is notifying its awaiter.
        let state = self.state.fetch_or(NOTIFYING, Ordering::AcqRel);

        // If the task was not notifying or registering an awaiter...
        if state & (NOTIFYING | REGISTERING) == 0 {
            // Take the waker out.
            let waker = unsafe { (*self.awaiter.get()).take() };

            // Unset the bit indicating that the task is notifying its awaiter.
            self.state
                .fetch_and(!NOTIFYING & !AWAITER, Ordering::Release);

            // Finally, notify the waker if it's different from the current waker.
            if let Some(w) = waker {
                match current {
                    None => return Some(w),
                    Some(c) if !w.will_wake(c) => return Some(w),
                    Some(_) => abort_on_panic(|| drop(w)),
                }
            }
        }

        None
    }

    /// Registers a new awaiter blocked on this task.
    ///
    /// This method is called when `Task` is polled and it has not yet completed.
    #[inline]
    pub(crate) fn register(&self, waker: &Waker) {
        // Load the state and synchronize with it.
        let mut state = self.state.fetch_or(0, Ordering::Acquire);

        loop {
            // There can't be two concurrent registrations because `Task` can only be polled
            // by a unique pinned reference.
            debug_assert!(state & REGISTERING == 0);

            // If we're in the notifying state at this moment, just wake and return without
            // registering.
            if state & NOTIFYING != 0 {
                abort_on_panic(|| waker.wake_by_ref());
                return;
            }

            // Mark the state to let other threads know we're registering a new awaiter.
            match self.state.compare_exchange_weak(
                state,
                state | REGISTERING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    state |= REGISTERING;
                    break;
                }
                Err(s) => state = s,
            }
        }

        // Put the waker into the awaiter field.
        unsafe {
            abort_on_panic(|| (*self.awaiter.get()) = Some(waker.clone()));
        }

        // This variable will contain the newly registered waker if a notification comes in before
        // we complete registration.
        let mut waker = None;

        loop {
            // If there was a notification, take the waker out of the awaiter field.
            if state & NOTIFYING != 0 {
                if let Some(w) = unsafe { (*self.awaiter.get()).take() } {
                    abort_on_panic(|| waker = Some(w));
                }
            }

            // The new state is not being notified nor registered, but there might or might not be
            // an awaiter depending on whether there was a concurrent notification.
            let new = if waker.is_none() {
                (state & !NOTIFYING & !REGISTERING) | AWAITER
            } else {
                state & !NOTIFYING & !REGISTERING & !AWAITER
            };

            match self
                .state
                .compare_exchange_weak(state, new, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(s) => state = s,
            }
        }

        // If there was a notification during registration, wake the awaiter now.
        if let Some(w) = waker {
            abort_on_panic(|| w.wake());
        }
    }

    /// Clones a waker.
    pub(crate) unsafe fn clone_waker(&self, vtable: &'static RawWakerVTable) -> RawWaker {
        let ptr: *const Header = self;
        let ptr: *const () = ptr.cast();

        // Increment the reference count. With any kind of reference-counted data structure,
        // relaxed ordering is appropriate when incrementing the counter.
        let state = self.state.fetch_add(REFERENCE, Ordering::Relaxed);

        // If the reference count overflowed, abort.
        if state > isize::MAX as usize {
            abort();
        }

        RawWaker::new(ptr, vtable)
    }

    #[inline(never)]
    pub(crate) unsafe fn drop_waker(&self) -> Option<Action> {
        // Decrement the reference count.
        let new = self.state.fetch_sub(REFERENCE, Ordering::AcqRel) - REFERENCE;

        // If this was the last reference to the task and the `Task` has been dropped too,
        // then we need to decide how to destroy the task.
        if new & !(REFERENCE - 1) == 0 && new & TASK == 0 {
            if new & (COMPLETED | CLOSED) == 0 {
                // If the task was not completed nor closed, close it and schedule one more time so
                // that its future gets dropped by the executor.
                self.state
                    .store(SCHEDULED | CLOSED | REFERENCE, Ordering::Release);
                Some(Action::Schedule)
            } else {
                // Otherwise, destroy the task right away.
                Some(Action::Destroy)
            }
        } else {
            None
        }
    }

    /// Puts the task in detached state.
    #[inline(never)]
    pub(crate) fn set_detached(&self) -> Option<*const ()> {
        let ptr: *const Header = self;
        let ptr: *const () = ptr.cast();

        unsafe {
            // A place where the output will be stored in case it needs to be dropped.
            let mut output = None;

            // Optimistically assume the `Task` is being detached just after creating the task.
            // This is a common case so if the `Task` is datached, the overhead of it is only one
            // compare-exchange operation.
            if let Err(mut state) = self.state.compare_exchange_weak(
                SCHEDULED | TASK | REFERENCE,
                SCHEDULED | REFERENCE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                loop {
                    // If the task has been completed but not yet closed, that means its output
                    // must be dropped.
                    if state & COMPLETED != 0 && state & CLOSED == 0 {
                        // Mark the task as closed in order to grab its output.
                        match self.state.compare_exchange_weak(
                            state,
                            state | CLOSED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(_) => {
                                // Read the output.
                                output = Some(self.vtable.get_output(ptr));

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
                            state & !TASK
                        };

                        // Unset the `TASK` flag.
                        match self.state.compare_exchange_weak(
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
                                        (self.vtable.schedule)(ptr, ScheduleInfo::new(false));
                                    } else {
                                        (self.vtable.destroy)(ptr);
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

    /// Puts the task in canceled state.
    #[inline(never)]
    pub(crate) fn set_canceled(&self) {
        let ptr: *const Header = self;
        let ptr: *const () = ptr.cast();

        unsafe {
            let mut state = self.state.load(Ordering::Acquire);

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
                match self.state.compare_exchange_weak(
                    state,
                    new,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        // If the task is not scheduled nor running, schedule it one more time so
                        // that its future gets dropped by the executor.
                        if state & (SCHEDULED | RUNNING) == 0 {
                            (self.vtable.schedule)(ptr, ScheduleInfo::new(false));
                        }

                        // Notify the awaiter that the task has been closed.
                        if state & AWAITER != 0 {
                            self.notify(None);
                        }

                        break;
                    }
                    Err(s) => state = s,
                }
            }
        }
    }
}

/// The header of a task.
///
/// This header is stored in memory at the beginning of the heap-allocated task.
#[repr(C)]
pub(crate) struct HeaderWithMetadata<M> {
    pub(crate) header: Header,

    /// Metadata associated with the task.
    ///
    /// This metadata may be provided to the user.
    pub(crate) metadata: M,
}

impl<M: fmt::Debug> fmt::Debug for HeaderWithMetadata<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.header.state.load(Ordering::SeqCst);

        f.debug_struct("Header")
            .field("scheduled", &(state & SCHEDULED != 0))
            .field("running", &(state & RUNNING != 0))
            .field("completed", &(state & COMPLETED != 0))
            .field("closed", &(state & CLOSED != 0))
            .field("awaiter", &(state & AWAITER != 0))
            .field("task", &(state & TASK != 0))
            .field("ref_count", &(state / REFERENCE))
            .field("metadata", &self.metadata)
            .finish()
    }
}
