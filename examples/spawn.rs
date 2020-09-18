//! A simple single-threaded executor.

use std::future::Future;
use std::panic::catch_unwind;
use std::thread;

use async_task::{Runnable, Task};
use futures_lite::future;
use once_cell::sync::Lazy;

/// Spawns a future on the executor.
fn spawn<F, T>(future: F) -> Task<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    // A channel that holds scheduled tasks.
    static QUEUE: Lazy<flume::Sender<Runnable>> = Lazy::new(|| {
        let (sender, receiver) = flume::unbounded::<Runnable>();

        // Start the executor thread.
        thread::spawn(|| {
            for runnable in receiver {
                // Ignore panics for simplicity.
                let _ignore_panic = catch_unwind(|| runnable.run());
            }
        });

        sender
    });

    // Create a task that is scheduled by sending itself into the channel.
    let schedule = |t| QUEUE.send(t).unwrap();
    let (runnable, task) = async_task::spawn(future, schedule);

    // Schedule the task by sending it into the channel.
    runnable.schedule();

    task
}

fn main() {
    // Spawn a future and await its result.
    let task = spawn(async {
        println!("Hello, world!");
    });
    future::block_on(task);
}
