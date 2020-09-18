//! A simple single-threaded executor that can spawn non-`Send` futures.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use async_task::{JoinHandle, Task};

thread_local! {
    // A channel that holds scheduled tasks.
    static QUEUE: (flume::Sender<Task>, flume::Receiver<Task>) = flume::unbounded();
}

/// Spawns a future on the executor.
fn spawn<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    // Create a task that is scheduled by sending itself into the channel.
    let schedule = |t| QUEUE.with(|(s, _)| s.send(t).unwrap());
    let (task, handle) = async_task::spawn_local(future, schedule);

    // Schedule the task by sending it into the queue.
    task.schedule();

    handle
}

/// Runs a future to completion.
fn run<F, T>(future: F) -> T
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    // Spawn a task that sends its result through a channel.
    let (s, r) = flume::unbounded();
    spawn(async move { drop(s.send(future.await)) }).detach();

    loop {
        // If the original task has completed, return its result.
        if let Ok(val) = r.try_recv() {
            return val;
        }

        // Otherwise, take a task from the queue and run it.
        QUEUE.with(|(_, r)| r.recv().unwrap().run());
    }
}

fn main() {
    let val = Rc::new(Cell::new(0));

    // Run a future that increments a non-`Send` value.
    run({
        let val = val.clone();
        async move {
            // Spawn a future that increments the value.
            let handle = spawn({
                let val = val.clone();
                async move {
                    val.set(dbg!(val.get()) + 1);
                }
            });

            val.set(dbg!(val.get()) + 1);
            handle.await;
        }
    });

    // The value should be 2 at the end of the program.
    dbg!(val.get());
}
