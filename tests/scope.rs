#![cfg(feature = "scope")]

use std::sync::Arc;
use std::thread;

use async_task::Runnable;
use concurrent_queue::ConcurrentQueue;
use smol::future;

struct Executor(concurrent_queue::ConcurrentQueue<Runnable<()>>);

impl Executor {
    fn run(&self) {
        while let Ok(runnable) = self.0.pop() {
            runnable.run();
        }
    }
}

#[test]
fn smoke() {
    // Some non-trivial outside data to borrow.
    let mut string = String::from("hello");

    future::block_on(async_task::scope(|scope| {
        let string = &mut string;

        let executor = Arc::new(Executor(ConcurrentQueue::unbounded()));
        let schedule = {
            let executor = executor.clone();
            move |runnable| {
                executor.0.push(runnable).ok();
            }
        };

        let (runnable, task) = scope.spawn(
            async move {
                string.push_str(" world!");
            },
            schedule,
        );
        runnable.schedule();

        async move {
            executor.run();
            task.await;
        }
    }));

    assert_eq!(string, "hello world!");
}

#[test]
fn future_cancelled() {
    let mut string = String::from("hello");

    future::block_on(async_task::scope(|scope| {
        let string = &mut string;

        let executor = Arc::new(Executor(ConcurrentQueue::unbounded()));
        let schedule = {
            let executor = executor.clone();
            move |runnable| {
                executor.0.push(runnable).ok();
            }
        };

        let (runnable, task) = scope.spawn(
            async move {
                string.push_str(" world!");
                future::pending::<()>().await;
            },
            schedule,
        );
        runnable.schedule();

        thread::spawn(move || executor.run());

        async move {
            task.cancel().await;
        }
    }));

    assert_eq!(string, "hello");
}

#[test]
fn task_sent_to_other_thread() {
    let mut string = String::from("hello");

    future::block_on(async_task::scope(|scope| {
        let string = &mut string;

        let executor = Arc::new(Executor(ConcurrentQueue::unbounded()));
        let schedule = {
            let executor = executor.clone();
            move |runnable| {
                executor.0.push(runnable).ok();
            }
        };

        let (runnable, task) = scope.spawn(
            async move {
                string.push_str(" world!");
            },
            schedule,
        );
        runnable.schedule();

        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(200));

            executor.run();
            future::block_on(task);
        });

        future::ready(())
    }));

    assert_eq!(string, "hello world!");
}
