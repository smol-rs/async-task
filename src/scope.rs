//! Scoped tasks, similar to scoped threads from crossbeam.

use crate::header::Header;
use crate::state::*;
use crate::utils::{abort, abort_on_panic_future};
use crate::{Builder, Runnable, Task};

use async_channel::{Receiver, Sender};
use concurrent_queue::ConcurrentQueue;

use alloc::collections::btree_map::{BTreeMap, Entry};

use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

impl<M> Builder<M> {
    /// Spawns a new task into a task-spawning [`scope`].
    ///
    /// See the documentation of [`scope`] for more details.
    pub fn spawn_scoped<'scope, 'env, F, Fut, S>(
        self,
        scope: &'scope Scope<'env, M>,
        future: F,
        schedule: S,
    ) -> (Runnable<M>, Task<Fut::Output, M>)
    where
        F: FnOnce(&M) -> Fut,
        Fut: Future + Send + 'scope + 'env,
        Fut::Output: Send + 'scope + 'env,
        S: Fn(Runnable<M>) + Send + Sync + 'static,
    {
        // Create a unique ID for the task.
        let id = scope.next_id.fetch_add(1, Ordering::SeqCst);

        // Create a future that wraps the current one, and also signals the scope when it is complete.
        let future = move |metadata| {
            // After the future has completed (panic or not), signal the scope.
            struct SignalScope<'scope, 'env, M> {
                scope: &'scope Scope<'env, M>,
                id: usize,
            }

            impl<M> Drop for SignalScope<'_, '_, M> {
                fn drop(&mut self) {
                    // Notify the scope that the task is complete.
                    self.scope.completion_channel.0.send_blocking(self.id).ok();
                }
            }

            let fut = future(metadata);
            async move {
                let _signal_scope = SignalScope { scope, id };
                fut.await
            }
        };

        // Spawn the task and add it to our list of tasks.
        let (runnable, task) = unsafe { self.spawn_unchecked(future, schedule) };
        scope.push(id, &task);
        (runnable, task)
    }
}

/// Creates a new scope for spawning tasks.
///
/// This function provides a safe way for tasks to access borrowed variables on the stack. In order to
/// prevent a use-after-free (e.g. the task outliving the scope), the scope will not return until all
/// tasks spawned within it have completed. This is similar to the [`scope`] function from the
/// [`crossbeam`] crate.
///
/// This function is only available when the `scope` feature is enabled.
///
/// [`scope`]: https://docs.rs/crossbeam-utils/latest/crossbeam_utils/thread/index.html
/// [`crossbeam`]: https://crates.io/crates/crossbeam
///
/// # Notes
///
/// For users of [`async_executor`]: this function is unnecessary, since the [`Executor`] struct
/// is already lifetime-aware.
///
/// [`async_executor`]: https://crates.io/crates/async-executor
/// [`Executor`]: https://docs.rs/async-executor/latest/async_executor/struct.Executor.html
///
/// # Example
///
/// ```rust
/// # smol::future::block_on(async {
/// // We have a list to do something with.
/// let list = vec!["Alice", "Bob", "Ronald"];
/// let mut my_string = String::from("hello");
///
/// // First, create a simple executor.
/// let (sender, receiver) = flume::unbounded();
/// let schedule = move |runnable| sender.send(runnable).unwrap();
///
/// // Then, create a scope to spawn tasks into.
/// let scoped = async_task::scope(|scope| {
///     // Note that, due to Rust's borrow checker limitations, we keep the task spawning
///     // proper outside of the `async` block.
///     let my_string = &mut my_string;
///
///     // Then, we spawn some tasks.
///     let mut tasks = Vec::new();
///     for name in &list {
///         let (runnable, task) = scope.spawn(async move {
///             println!("Hello, {}!", name);
///         }, schedule.clone());
///
///         runnable.schedule();
///         tasks.push(task);
///     }
///
///     // We can also use task builders.
///     // The only restriction is that all tasks in a scope must use the same metadata.
///     let (runnable, other_task) = async_task::Builder::new()
///         .propagate_panic(true)
///         .spawn_scoped(scope, |()| async move {
///             my_string.push_str(" world");
///         }, schedule.clone());
///     runnable.schedule();
///     tasks.push(other_task);
///
///     // Finally, we wait for all tasks to complete.
///     async move {
///         while let Ok(runnable) = receiver.try_recv() {
///             runnable.run();
///         }
///
///         for task in tasks {
///             task.await;
///         }
///     }
/// });
///
/// // The scope is a future itself and must be awaited.
/// scoped.await;
///
/// assert_eq!(my_string, "hello world");
/// # });
/// ```
pub async fn scope<'env, Fut: Future, M: 'env>(
    f: impl FnOnce(&Scope<'env, M>) -> Fut,
) -> Fut::Output {
    // Create a new scope
    let scope = Scope {
        tasks: ConcurrentQueue::unbounded(),
        completion_channel: async_channel::unbounded(),
        next_id: AtomicUsize::new(0),
        _marker: PhantomData,
    };

    // Create and run the future using the scope.
    let result = f(&scope).await;

    // Join all tasks spawned in the scope.
    scope.join().await;

    // SAFETY: All tasks have been joined, so no variables are left borrowed.

    // Return the result of the future.
    result
}

/// A scope that can be used to spawn scoped tasks.
///
/// See the [`scope`] function for more details.
pub struct Scope<'env, M> {
    /// Pointers to the tasks that we have spawned.
    tasks: ConcurrentQueue<(usize, CompleteHandle<M>)>,

    /// A channel used to signal that an operation is complete.
    ///
    /// Ideally, we'd just use events with tags in them, but the API for that is still being
    /// decided. See <https://github.com/smol-rs/event-listener/pull/40>. For now, we just use
    /// a channel.
    completion_channel: (Sender<usize>, Receiver<usize>),

    /// Generate new IDs for tasks.
    next_id: AtomicUsize,

    /// Capture an invariant lifetime and the metadata.
    _marker: PhantomData<&'env mut &'env M>,
}

impl<M> fmt::Debug for Scope<'_, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope")
            .field("num_tasks", &self.tasks.len())
            .finish_non_exhaustive()
    }
}

unsafe impl<M: Send + Sync> Sync for Scope<'_, M> {}

impl<'env, M> Scope<'env, M> {
    /// Pushes a task into the scope.
    fn push<R>(&self, id: usize, task: &Task<R, M>) {
        self.tasks
            .push((id, CompleteHandle::new(task)))
            .ok()
            .expect("Scope context already dropped");
    }

    /// Join all of the handles in the scope.
    fn join(self) -> impl Future<Output = ()> + 'env {
        // A panic here would leave the program in an invalid state.
        abort_on_panic_future(async move {
            // Close the queue to prevent more tasks from being spawned.
            self.tasks.close();

            // Have a local ring buffer of tasks that we are waiting on.
            let mut tasks = BTreeMap::new();

            // Iterate through the tasks that the user spawned.
            while let Ok((id, task)) = self.tasks.pop() {
                // See if the task is complete.
                if task.is_complete() {
                    // If it is, drop it.
                    drop(task);
                } else {
                    // Otherwise, add it to the list of tasks to wait on.
                    tasks.insert(id, task);
                }
            }

            // Wait until all of the pending tasks are complete.
            while !tasks.is_empty() {
                // Wait for a task to complete.
                let id = match self.completion_channel.1.recv().await {
                    Ok(id) => id,
                    Err(_) => {
                        // All senders are dropped, implying all futures are complete.
                        break;
                    }
                };

                // See if the task is complete.
                if let Entry::Occupied(entry) = tasks.entry(id) {
                    // If it is, drop it.
                    drop(entry.remove());
                }
            }
        })
    }
}

impl<'env> Scope<'env, ()> {
    /// Spawn a new task into the scope.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # smol::future::block_on(async {
    /// let mut i = 32;
    ///
    /// async_task::scope(|s| {
    ///     let (runnable, task) = s.spawn(async {
    ///         i += 1;
    ///     }, |_| {});
    ///     runnable.run();
    ///
    ///     async move {
    ///         task.await;
    ///     }
    /// }).await;
    ///
    /// assert_eq!(i, 33);
    /// # });
    /// ```
    pub fn spawn<Fut, S>(&self, future: Fut, schedule: S) -> (Runnable<()>, Task<Fut::Output, ()>)
    where
        Fut: Future + Send + 'env,
        Fut::Output: Send + 'env,
        S: Fn(Runnable<()>) + Send + Sync + 'static,
    {
        Builder::new().spawn_scoped(self, move |()| future, schedule)
    }
}

/// A handle for a task used to probe for completion
struct CompleteHandle<M> {
    /// The header of the task.
    header: NonNull<Header<M>>,
}

unsafe impl<M: Send + Sync> Send for CompleteHandle<M> {}
unsafe impl<M: Send + Sync> Sync for CompleteHandle<M> {}

impl<M> CompleteHandle<M> {
    /// Create a new completion handle from a task.
    fn new<T>(task: &Task<T, M>) -> Self {
        let ptr: NonNull<Header<M>> = task.ptr.cast();

        unsafe {
            // Increment the reference counter.
            let state = ptr.as_ref().state.fetch_add(REFERENCE, Ordering::Relaxed);

            // If the reference count may overflow, abort.
            // The reference count can never be zero, since we hold a reference to the Task.
            if state > core::isize::MAX as usize {
                abort();
            }
        }

        Self { header: ptr }
    }

    /// Tell whether the task is complete.
    fn is_complete(&self) -> bool {
        let state = unsafe { self.header.as_ref().state.load(Ordering::SeqCst) };

        // The task will be CLOSED & !SCHEDULED if it is complete.
        state & (CLOSED | SCHEDULED) == CLOSED
    }
}

impl<M> Drop for CompleteHandle<M> {
    fn drop(&mut self) {
        // Decrement the reference counter, potentially dropping the task.
        unsafe {
            (self.header.as_ref().vtable.drop_ref)(self.header.as_ptr().cast());
        }
    }
}
