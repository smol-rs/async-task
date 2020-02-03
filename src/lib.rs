//! Task abstraction for building executors.
//!
//! # Spawning
//!
//! To spawn a future onto an executor, we first need to allocate it on the heap and keep some
//! state alongside it. The state indicates whether the future is ready for polling, waiting to be
//! woken up, or completed. Such a future is called a *task*.
//!
//! All executors have some kind of queue that holds runnable tasks:
//!
//! ```
//! let (sender, receiver) = crossbeam::channel::unbounded();
//! #
//! # // A future that will get spawned.
//! # let future = async { 1 + 2 };
//! #
//! # // A function that schedules the task when it gets woken up.
//! # let schedule = move |task| sender.send(task).unwrap();
//! #
//! # // Construct a task.
//! # let (task, handle) = async_task::spawn(future, schedule, ());
//! ```
//!
//! A task is constructed using either [`spawn`] or [`spawn_local`]:
//!
//! ```
//! # let (sender, receiver) = crossbeam::channel::unbounded();
//! #
//! // A future that will be spawned.
//! let future = async { 1 + 2 };
//!
//! // A function that schedules the task when it gets woken up.
//! let schedule = move |task| sender.send(task).unwrap();
//!
//! // Construct a task.
//! let (task, handle) = async_task::spawn(future, schedule, ());
//!
//! // Push the task into the queue by invoking its schedule function.
//! task.schedule();
//! ```
//!
//! The last argument to the [`spawn`] function is a *tag*, an arbitrary piece of data associated
//! with the task. In most executors, this is typically a task identifier or task-local storage.
//!
//! The function returns a runnable [`Task`] and a [`JoinHandle`] that can await the result.
//!
//! # Execution
//!
//! Task executors have some kind of main loop that drives tasks to completion. That means taking
//! runnable tasks out of the queue and running each one in order:
//!
//! ```no_run
//! # let (sender, receiver) = crossbeam::channel::unbounded();
//! #
//! # // A future that will get spawned.
//! # let future = async { 1 + 2 };
//! #
//! # // A function that schedules the task when it gets woken up.
//! # let schedule = move |task| sender.send(task).unwrap();
//! #
//! # // Construct a task.
//! # let (task, handle) = async_task::spawn(future, schedule, ());
//! #
//! # // Push the task into the queue by invoking its schedule function.
//! # task.schedule();
//! #
//! for task in receiver {
//!     task.run();
//! }
//! ```
//!
//! When a task is run, its future gets polled. If polling does not complete the task, that means
//! it's waiting for another future and needs to go to sleep. When woken up, its schedule function
//! will be invoked, pushing it back into the queue so that it can be run again.
//!
//! # Cancellation
//!
//! Both [`Task`] and [`JoinHandle`] have methods that cancel the task. When cancelled, the task's
//! future will not be polled again and will get dropped instead.
//!
//! If cancelled by the [`Task`] instance, the task is destroyed immediately. If cancelled by the
//! [`JoinHandle`] instance, it will be scheduled one more time and the next attempt to run it will
//! simply destroy it.
//!
//! # Performance
//!
//! Task construction incurs a single allocation that holds its state, the schedule function, and
//! the future or the result of the future if completed.
//!
//! The layout of a task is equivalent to 4 `usize`s followed by the schedule function, and then by
//! a union of the future and its output.
//!
//! # Waking
//!
//! The handy [`waker_fn`] constructor converts any function into a [`Waker`]. Every time it is
//! woken, the function gets called:
//!
//! ```
//! let waker = async_task::waker_fn(|| println!("Wake!"));
//!
//! // Prints "Wake!" twice.
//! waker.wake_by_ref();
//! waker.wake_by_ref();
//! ```
//!
//! This is useful for implementing single-future executors like [`block_on`].
//!
//! [`spawn`]: fn.spawn.html
//! [`spawn_local`]: fn.spawn_local.html
//! [`waker_fn`]: fn.waker_fn.html
//! [`Task`]: struct.Task.html
//! [`JoinHandle`]: struct.JoinHandle.html
//! [`Waker`]: https://doc.rust-lang.org/std/task/struct.Waker.html
//! [`block_on`]: https://github.com/async-rs/async-task/blob/master/examples/block.rs

#![no_std]
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
#![doc(test(attr(deny(rust_2018_idioms, warnings))))]
#![doc(test(attr(allow(unused_extern_crates, unused_variables))))]

extern crate alloc;

mod header;
mod join_handle;
mod raw;
mod state;
mod task;
mod utils;
mod waker_fn;

pub use crate::join_handle::JoinHandle;
pub use crate::task::{spawn, Task};
pub use crate::waker_fn::waker_fn;

#[cfg(any(unix, windows))]
pub use crate::task::spawn_local;
