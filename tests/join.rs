use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use async_task::Task;
use easy_parallel::Parallel;
use futures_lite::future;

// Creates a future with event counters.
//
// Usage: `future!(f, POLL, DROP_F, DROP_T)`
//
// The future `f` outputs `Poll::Ready`.
// When it gets polled, `POLL` is incremented.
// When it gets dropped, `DROP_F` is incremented.
// When the output gets dropped, `DROP_T` is incremented.
macro_rules! future {
    ($name:pat, $poll:ident, $drop_f:ident, $drop_t:ident) => {
        static $poll: AtomicUsize = AtomicUsize::new(0);
        static $drop_f: AtomicUsize = AtomicUsize::new(0);
        static $drop_t: AtomicUsize = AtomicUsize::new(0);

        let $name = {
            struct Fut(Box<i32>);

            impl Future for Fut {
                type Output = Out;

                fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                    $poll.fetch_add(1, Ordering::SeqCst);
                    Poll::Ready(Out(Box::new(0)))
                }
            }

            impl Drop for Fut {
                fn drop(&mut self) {
                    $drop_f.fetch_add(1, Ordering::SeqCst);
                }
            }

            struct Out(Box<i32>);

            impl Drop for Out {
                fn drop(&mut self) {
                    $drop_t.fetch_add(1, Ordering::SeqCst);
                }
            }

            Fut(Box::new(0))
        };
    };
}

// Creates a schedule function with event counters.
//
// Usage: `schedule!(s, SCHED, DROP)`
//
// The schedule function `s` does nothing.
// When it gets invoked, `SCHED` is incremented.
// When it gets dropped, `DROP` is incremented.
macro_rules! schedule {
    ($name:pat, $sched:ident, $drop:ident) => {
        static $drop: AtomicUsize = AtomicUsize::new(0);
        static $sched: AtomicUsize = AtomicUsize::new(0);

        let $name = {
            struct Guard(Box<i32>);

            impl Drop for Guard {
                fn drop(&mut self) {
                    $drop.fetch_add(1, Ordering::SeqCst);
                }
            }

            let guard = Guard(Box::new(0));
            move |task: Task| {
                &guard;
                task.schedule();
                $sched.fetch_add(1, Ordering::SeqCst);
            }
        };
    };
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn drop_and_join() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    drop(task);
    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    assert!(future::block_on(handle).is_none());
    assert_eq!(POLL.load(Ordering::SeqCst), 0);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);
}

#[test]
fn run_and_join() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    task.run();
    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    assert!(future::block_on(handle).is_some());
    assert_eq!(POLL.load(Ordering::SeqCst), 1);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_T.load(Ordering::SeqCst), 1);
}

#[test]
fn detach_and_run() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    handle.detach();
    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    task.run();
    assert_eq!(POLL.load(Ordering::SeqCst), 1);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_T.load(Ordering::SeqCst), 1);
}

#[test]
fn join_twice() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, mut handle) = async_task::spawn(f, s);

    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    task.run();
    assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

    assert!(future::block_on(&mut handle).is_some());
    assert_eq!(POLL.load(Ordering::SeqCst), 1);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_T.load(Ordering::SeqCst), 1);

    assert!(future::block_on(&mut handle).is_none());
    assert_eq!(POLL.load(Ordering::SeqCst), 1);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_T.load(Ordering::SeqCst), 1);

    handle.detach();
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
}

#[test]
fn join_and_cancel() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    Parallel::new()
        .add(|| {
            thread::sleep(ms(200));
            drop(task);

            thread::sleep(ms(400));
            assert_eq!(POLL.load(Ordering::SeqCst), 0);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
        })
        .add(|| {
            assert!(future::block_on(handle).is_none());
            assert_eq!(POLL.load(Ordering::SeqCst), 0);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);

            thread::sleep(ms(200));
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
        })
        .run();
}

#[test]
fn join_and_run() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    Parallel::new()
        .add(|| {
            thread::sleep(ms(400));

            task.run();
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);

            thread::sleep(ms(200));
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
        })
        .add(|| {
            assert!(future::block_on(handle).is_some());
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 1);

            thread::sleep(ms(200));
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
        })
        .run();
}

#[test]
fn try_join_and_run_and_join() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, mut handle) = async_task::spawn(f, s);

    Parallel::new()
        .add(|| {
            thread::sleep(ms(400));

            task.run();
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);

            thread::sleep(ms(200));
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
        })
        .add(|| {
            future::block_on(future::or(&mut handle, future::ready(Default::default())));
            assert_eq!(POLL.load(Ordering::SeqCst), 0);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

            assert!(future::block_on(handle).is_some());
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 1);

            thread::sleep(ms(200));
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
        })
        .run();
}

#[test]
fn try_join_and_cancel_and_run() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, mut handle) = async_task::spawn(f, s);

    Parallel::new()
        .add(|| {
            thread::sleep(ms(200));

            task.run();
            assert_eq!(POLL.load(Ordering::SeqCst), 0);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
        })
        .add(|| {
            future::block_on(future::or(&mut handle, future::ready(Default::default())));
            assert_eq!(POLL.load(Ordering::SeqCst), 0);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

            drop(handle);
            assert_eq!(POLL.load(Ordering::SeqCst), 0);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 0);
        })
        .run();
}

#[test]
fn try_join_and_run_and_cancel() {
    future!(f, POLL, DROP_F, DROP_T);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, mut handle) = async_task::spawn(f, s);

    Parallel::new()
        .add(|| {
            thread::sleep(ms(200));

            task.run();
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
        })
        .add(|| {
            future::block_on(future::or(&mut handle, future::ready(Default::default())));
            assert_eq!(POLL.load(Ordering::SeqCst), 0);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 0);

            thread::sleep(ms(400));

            drop(handle);
            assert_eq!(POLL.load(Ordering::SeqCst), 1);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_T.load(Ordering::SeqCst), 1);
        })
        .run();
}

#[test]
fn await_output() {
    struct Fut<T>(Cell<Option<T>>);

    impl<T> Fut<T> {
        fn new(t: T) -> Fut<T> {
            Fut(Cell::new(Some(t)))
        }
    }

    impl<T> Future for Fut<T> {
        type Output = T;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Ready(self.0.take().unwrap())
        }
    }

    for i in 0..10 {
        let (task, handle) = async_task::spawn(Fut::new(i), drop);
        task.run();
        assert_eq!(future::block_on(handle), Some(i));
    }

    for i in 0..10 {
        let (task, handle) = async_task::spawn(Fut::new(vec![7; i]), drop);
        task.run();
        assert_eq!(future::block_on(handle), Some(vec![7; i]));
    }

    let (task, handle) = async_task::spawn(Fut::new("foo".to_string()), drop);
    task.run();
    assert_eq!(future::block_on(handle), Some("foo".to_string()));
}
