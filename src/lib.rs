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
//! let (sender, receiver) = flume::unbounded();
//! #
//! # // A future that will get spawned.
//! # let future = async { 1 + 2 };
//! #
//! # // A function that schedules the task when it gets woken up.
//! # let schedule = move |runnable| sender.send(runnable).unwrap();
//! #
//! # // Construct a task.
//! # let (runnable, task) = async_task::spawn(future, schedule);
//! ```
//!
//! A task is constructed using either [`spawn`] or [`spawn_local`]:
//!
//! ```
//! # let (sender, receiver) = flume::unbounded();
//! #
//! // A future that will be spawned.
//! let future = async { 1 + 2 };
//!
//! // A function that schedules the task when it gets woken up.
//! let schedule = move |runnable| sender.send(runnable).unwrap();
//!
//! // Construct a task.
//! let (runnable, task) = async_task::spawn(future, schedule);
//!
//! // Push the task into the queue by invoking its schedule function.
//! runnable.schedule();
//! ```
//!
//! The function returns a runnable [`Runnable`] and a [`Task`] that can await the result.
//!
//! # Execution
//!
//! Task executors have some kind of main loop that drives tasks to completion. That means taking
//! runnable tasks out of the queue and running each one in order:
//!
//! ```no_run
//! # let (sender, receiver) = flume::unbounded();
//! #
//! # // A future that will get spawned.
//! # let future = async { 1 + 2 };
//! #
//! # // A function that schedules the task when it gets woken up.
//! # let schedule = move |runnable| sender.send(runnable).unwrap();
//! #
//! # // Construct a task.
//! # let (runnable, task) = async_task::spawn(future, schedule);
//! #
//! # // Push the task into the queue by invoking its schedule function.
//! # runnable.schedule();
//! #
//! for runnable in receiver {
//!     runnable.run();
//! }
//! ```
//!
//! When a task is run, its future gets polled. If polling does not complete the task, that means
//! it's waiting for another future and needs to go to sleep. When woken up, its schedule function
//! will be invoked, pushing it back into the queue so that it can be run again.
//!
//! # Cancellation
//!
//! Both [`Runnable`] and [`Task`] have methods that cancel the task. When canceled, the
//! task's future will not be polled again and will get dropped instead.
//!
//! If canceled by the [`Runnable`] instance, the task is destroyed immediately. If canceled by the
//! [`Task`] instance, it will be scheduled one more time and the next attempt to run it will
//! simply destroy it.
//!
//! The `Task` future will then evaluate to `None`, but only after the task's future is
//! dropped.
//!
//! # Performance
//!
//! Task construction incurs a single allocation that holds its state, the schedule function, and
//! the future or the result of the future if completed.
//!
//! The layout of a task is equivalent to 4 `usize`s followed by the schedule function, and then by
//! a union of the future and its output.
//!
//! [`block_on`]: https://github.com/stjepang/async-task/blob/master/examples/block.rs

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
#![doc(test(attr(deny(rust_2018_idioms, warnings))))]
#![doc(test(attr(allow(unused_extern_crates, unused_variables))))]

extern crate alloc;

mod header;
mod raw;
mod runnable;
mod state;
mod task;
mod utils;

pub use crate::runnable::{spawn, Runnable};
pub use crate::task::Task;

#[cfg(feature = "std")]
pub use crate::runnable::spawn_local;
