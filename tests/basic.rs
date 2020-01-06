use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use async_task::Task;
use crossbeam::atomic::AtomicCell;
use crossbeam::channel;
use futures::future;
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

// Creates a task with event counters.
//
// Usage: `task!(task, handle f, s, DROP)`
//
// A task with future `f` and schedule function `s` is created.
// The `Task` and `JoinHandle` are bound to `task` and `handle`, respectively.
// When the tag inside the task gets dropped, `DROP` is incremented.
macro_rules! task {
    ($task:pat, $handle: pat, $future:expr, $schedule:expr, $drop:ident) => {
        lazy_static! {
            static ref $drop: AtomicCell<usize> = AtomicCell::new(0);
        }

        let ($task, $handle) = {
            struct Tag(Box<i32>);

            impl Drop for Tag {
                fn drop(&mut self) {
                    $drop.fetch_add(1);
                }
            }

            async_task::spawn($future, $schedule, Tag(Box::new(0)))
        };
    };
}

#[test]
fn cancel_and_drop_handle() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    task!(task, handle, f, s, DROP_T);

    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    task.cancel();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    drop(task);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(DROP_T.load(), 1);
}

#[test]
fn run_and_drop_handle() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    task!(task, handle, f, s, DROP_T);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(DROP_T.load(), 1);
}

#[test]
fn drop_handle_and_run() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    task!(task, handle, f, s, DROP_T);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(DROP_T.load(), 1);
}

#[test]
fn cancel_and_run() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    task!(task, handle, f, s, DROP_T);

    handle.cancel();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    drop(handle);
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    task.run();
    assert_eq!(POLL.load(), 0);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(DROP_T.load(), 1);
}

#[test]
fn run_and_cancel() {
    future!(f, POLL, DROP_F);
    schedule!(s, SCHEDULE, DROP_S);
    task!(task, handle, f, s, DROP_T);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    handle.cancel();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(DROP_T.load(), 0);

    drop(handle);
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(DROP_T.load(), 1);
}

#[test]
fn schedule() {
    let (s, r) = channel::unbounded();
    let schedule = move |t| s.send(t).unwrap();
    let (task, _handle) = async_task::spawn(
        future::poll_fn(|_| Poll::<()>::Pending),
        schedule,
        Box::new(0),
    );

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
fn tag() {
    let (s, r) = channel::unbounded();
    let schedule = move |t| s.send(t).unwrap();
    let (task, handle) = async_task::spawn(
        future::poll_fn(|_| Poll::<()>::Pending),
        schedule,
        AtomicUsize::new(7),
    );

    assert!(r.is_empty());
    task.schedule();

    let task = r.recv().unwrap();
    assert!(r.is_empty());
    handle.tag().fetch_add(1, Ordering::SeqCst);
    task.schedule();

    let task = r.recv().unwrap();
    assert_eq!(task.tag().load(Ordering::SeqCst), 8);
    assert!(r.is_empty());
    task.schedule();

    r.recv().unwrap();
}

#[test]
fn schedule_counter() {
    let (s, r) = channel::unbounded();
    let schedule = move |t: Task<AtomicUsize>| {
        t.tag().fetch_add(1, Ordering::SeqCst);
        s.send(t).unwrap();
    };
    let (task, handle) = async_task::spawn(
        future::poll_fn(|_| Poll::<()>::Pending),
        schedule,
        AtomicUsize::new(0),
    );
    task.schedule();

    assert_eq!(handle.tag().load(Ordering::SeqCst), 1);
    r.recv().unwrap().schedule();

    assert_eq!(handle.tag().load(Ordering::SeqCst), 2);
    r.recv().unwrap().schedule();

    assert_eq!(handle.tag().load(Ordering::SeqCst), 3);
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

    let (task, _) = async_task::spawn(
        async {},
        move |task| {
            assert_eq!(guard.0.load(Ordering::SeqCst), 0);
            drop(task);
            assert_eq!(guard.0.load(Ordering::SeqCst), 0);
        },
        (),
    );
    task.schedule();
}

#[test]
fn waker() {
    let (s, r) = channel::unbounded();
    let schedule = move |t| s.send(t).unwrap();
    let (task, handle) = async_task::spawn(
        future::poll_fn(|_| Poll::<()>::Pending),
        schedule,
        Box::new(0),
    );

    assert!(r.is_empty());
    let w = task.waker();
    task.run();
    w.wake();

    let task = r.recv().unwrap();
    task.run();
    handle.waker().wake();

    r.recv().unwrap();
}

#[test]
fn raw() {
    let (task, _handle) = async_task::spawn(async {}, |_| panic!(), Box::new(AtomicUsize::new(7)));

    let a = task.into_raw();
    let task = unsafe {
        (*a).fetch_add(1, Ordering::SeqCst);
        Task::from_raw(a)
    };

    assert_eq!(task.tag().load(Ordering::SeqCst), 8);
    task.run();
}
