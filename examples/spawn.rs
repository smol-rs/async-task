//! A simple single-threaded executor.

use std::future::Future;
use std::panic::catch_unwind;
use std::thread;

use crossbeam::channel::{unbounded, Sender};
use futures::executor;
use lazy_static::lazy_static;

type Task = async_task::Task<()>;
type JoinHandle<T> = async_task::JoinHandle<T, ()>;

/// Spawns a future on the executor.
fn spawn<F, R>(future: F) -> JoinHandle<R>
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    lazy_static! {
        // A channel that holds scheduled tasks.
        static ref QUEUE: Sender<Task> = {
            let (sender, receiver) = unbounded::<Task>();

            // Start the executor thread.
            thread::spawn(|| {
                for task in receiver {
                    // Ignore panics for simplicity.
                    let _ignore_panic = catch_unwind(|| task.run());
                }
            });

            sender
        };
    }

    // Create a task that is scheduled by sending itself into the channel.
    let schedule = |t| QUEUE.send(t).unwrap();
    let (task, handle) = async_task::spawn(future, schedule, ());

    // Schedule the task by sending it into the channel.
    task.schedule();

    handle
}

fn main() {
    // Spawn a future and await its result.
    let handle = spawn(async {
        println!("Hello, world!");
    });
    executor::block_on(handle);
}
