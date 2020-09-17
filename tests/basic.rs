use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use async_task::Task;
use crossbeam::atomic::AtomicCell;
use crossbeam::channel;
use futures::future::{self, FutureExt};
use lazy_static::lazy_static;

// Creates a future with event counters.
//
// Usage: `future!(f, POLL, DROP)`
//
// The future `f` always returns `Poll::Ready`.
// When it gets polled, `POLL` is incremented.
// When it gets dropped, `DROP` is incremented.
macro_rules! future {
    ($name:pat, $poll:ident, $drop:ident) => {
        lazy_static! {
            static ref $poll: AtomicCell<usize> = AtomicCell::new(0);
            static ref $drop: AtomicCell<usize> = AtomicCell::new(0);
        }

        let $name = {
            struct Fut(Box<i32>);

            impl Future for Fut {
                type Output = Box<i32>;

                fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                    $poll.fetch_add(1);
                    Poll::Ready(Box::new(0))
                }
            }

            impl Drop for Fut {
                fn drop(&mut self) {
                    $drop.fetch_add(1);
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
        lazy_static! {
            static ref $sched: AtomicCell<usize> = AtomicCell::new(0);
            static ref $drop: AtomicCell<usize> = AtomicCell::new(0);
        }

        let $name = {
            struct Guard(Box<i32>);

            impl Drop for Guard {
                fn drop(&mut self) {
                    $drop.fetch_add(1);
                }
            }

            let guard = Guard(Box::new(0));
            move |_task| {
                &guard;
                $sched.fetch_add(1);
            }
        };
    };
}

#[test]
fn cancel_and_drop_handle() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    task.cancel();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    drop(task);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
}

#[test]
fn run_and_drop_handle() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
}

#[test]
fn drop_handle_and_run() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
}

#[test]
fn cancel_and_run() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    handle.cancel();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    task.run();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
}

#[test]
fn run_and_cancel() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);

    handle.cancel();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);

    drop(handle);
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
}

#[test]
fn cancel_and_poll() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    handle.cancel();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);

    let mut handle = handle;
    assert!((&mut handle).now_or_never().is_none());

    task.run();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);

    assert!((&mut handle).now_or_never().is_some());
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
}

#[test]
fn schedule() {
    let (s, r) = channel::unbounded();
    let schedule = move |t| s.send(t).unwrap();
    let (task, _handle) = async_task::spawn(future::poll_fn(|_| Poll::<()>::Pending), schedule);

    assert!(r.is_empty());
    task.schedule();

    let task = r.recv().unwrap();
    assert!(r.is_empty());
    task.schedule();

    let task = r.recv().unwrap();
    assert!(r.is_empty());
    task.schedule();

    r.recv().unwrap();
}

#[test]
fn schedule_counter() {
    static COUNT: AtomicUsize = AtomicUsize::new(0);

    let (s, r) = channel::unbounded();
    let schedule = move |t: Task| {
        COUNT.fetch_add(1, Ordering::SeqCst);
        s.send(t).unwrap();
    };
    let (task, _handle) = async_task::spawn(future::poll_fn(|_| Poll::<()>::Pending), schedule);
    task.schedule();

    r.recv().unwrap().schedule();
    r.recv().unwrap().schedule();
    assert_eq!(COUNT.load(Ordering::SeqCst), 3);
    r.recv().unwrap();
}

#[test]
fn drop_inside_schedule() {
    struct DropGuard(AtomicUsize);
    impl Drop for DropGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let guard = DropGuard(AtomicUsize::new(0));

    let (task, _) = async_task::spawn(async {}, move |task| {
        assert_eq!(guard.0.load(Ordering::SeqCst), 0);
        drop(task);
        assert_eq!(guard.0.load(Ordering::SeqCst), 0);
    });
    task.schedule();
}

#[test]
fn waker() {
    let (s, r) = channel::unbounded();
    let schedule = move |t| s.send(t).unwrap();
    let (task, handle) = async_task::spawn(future::poll_fn(|_| Poll::<()>::Pending), schedule);

    assert!(r.is_empty());
    let w = task.waker();
    task.run();
    w.wake();

    let task = r.recv().unwrap();
    task.run();
    assert!(r.is_empty());
}

#[test]
fn raw() {
    let (task, _handle) = async_task::spawn(async {}, |_| panic!());

    let a = task.into_raw();
    let task = unsafe { Task::from_raw(a) };
    task.run();
}
