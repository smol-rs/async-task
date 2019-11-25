//! A simple single-threaded executor that can spawn non-`Send` futures.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use crossbeam::channel::{unbounded, Receiver, Sender};

type Task = async_task::Task<()>;
type JoinHandle<T> = async_task::JoinHandle<T, ()>;

thread_local! {
    // A channel that holds scheduled tasks.
    static QUEUE: (Sender<Task>, Receiver<Task>) = unbounded();
}

/// Spawns a future on the executor.
fn spawn<F, R>(future: F) -> JoinHandle<R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    // Create a task that is scheduled by sending itself into the channel.
    let schedule = |t| QUEUE.with(|(s, _)| s.send(t).unwrap());
    let (task, handle) = async_task::spawn_local(future, schedule, ());

    // Schedule the task by sending it into the queue.
    task.schedule();

    handle
}

/// Runs a future to completion.
fn run<F, R>(future: F) -> R
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    // Spawn a task that sends its result through a channel.
    let (s, r) = unbounded();
    spawn(async move { s.send(future.await).unwrap() });

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
