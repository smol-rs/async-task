use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::Waker;
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use async_task::Task;
use crossbeam::atomic::AtomicCell;
use crossbeam::channel;
use lazy_static::lazy_static;

// Creates a future with event counters.
//
// Usage: `future!(f, waker, POLL, DROP)`
//
// The future `f` always sleeps for 200 ms, and returns `Poll::Ready` the second time it is polled.
// When it gets polled, `POLL` is incremented.
// When it gets dropped, `DROP` is incremented.
//
// Every time the future is run, it stores the waker into a global variable.
// This waker can be extracted using the `waker` function.
macro_rules! future {
    ($name:pat, $waker:pat, $poll:ident, $drop:ident) => {
        lazy_static! {
            static ref $poll: AtomicCell<usize> = AtomicCell::new(0);
            static ref $drop: AtomicCell<usize> = AtomicCell::new(0);
            static ref WAKER: AtomicCell<Option<Waker>> = AtomicCell::new(None);
        }

        let ($name, $waker) = {
            struct Fut(Cell<bool>, Box<i32>);

            impl Future for Fut {
                type Output = Box<i32>;

                fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                    WAKER.store(Some(cx.waker().clone()));
                    $poll.fetch_add(1);
                    thread::sleep(ms(200));

                    if self.0.get() {
                        Poll::Ready(Box::new(0))
                    } else {
                        self.0.set(true);
                        Poll::Pending
                    }
                }
            }

            impl Drop for Fut {
                fn drop(&mut self) {
                    $drop.fetch_add(1);
                }
            }

            (Fut(Cell::new(false), Box::new(0)), || {
                WAKER.swap(None).unwrap()
            })
        };
    };
}

// Creates a schedule function with event counters.
//
// Usage: `schedule!(s, chan, SCHED, DROP)`
//
// The schedule function `s` pushes the task into `chan`.
// When it gets invoked, `SCHED` is incremented.
// When it gets dropped, `DROP` is incremented.
//
// Receiver `chan` extracts the task when it is scheduled.
macro_rules! schedule {
    ($name:pat, $chan:pat, $sched:ident, $drop:ident) => {
        lazy_static! {
            static ref $sched: AtomicCell<usize> = AtomicCell::new(0);
            static ref $drop: AtomicCell<usize> = AtomicCell::new(0);
        }

        let ($name, $chan) = {
            let (s, r) = channel::unbounded();

            struct Guard(Box<i32>);

            impl Drop for Guard {
                fn drop(&mut self) {
                    $drop.fetch_add(1);
                }
            }

            let guard = Guard(Box::new(0));
            let sched = move |task: Task| {
                &guard;
                $sched.fetch_add(1);
                s.send(task).unwrap();
            };

            (sched, r)
        };
    };
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn wake() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (mut task, _) = async_task::spawn(f, s);

    assert!(chan.is_empty());

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    waker().wake();
    task = chan.recv().unwrap();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    task.run();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    waker().wake();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(chan.len(), 0);
}

#[test]
fn wake_by_ref() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (mut task, _) = async_task::spawn(f, s);

    assert!(chan.is_empty());

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    waker().wake_by_ref();
    task = chan.recv().unwrap();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    task.run();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    waker().wake_by_ref();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(chan.len(), 0);
}

#[test]
fn clone() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (mut task, _) = async_task::spawn(f, s);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    let w2 = waker().clone();
    let w3 = w2.clone();
    let w4 = w3.clone();
    w4.wake();

    task = chan.recv().unwrap();
    task.run();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    w3.wake();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    drop(w2);
    drop(waker());
    assert_eq!(DROP_S.load(), 1);
}

#[test]
fn wake_canceled() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (task, _) = async_task::spawn(f, s);

    task.run();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    let w = waker();

    w.wake_by_ref();
    chan.recv().unwrap().cancel();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    w.wake();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(chan.len(), 0);
}

#[test]
fn wake_completed() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (task, _) = async_task::spawn(f, s);

    task.run();
    let w = waker();
    assert_eq!(POLL.load(), 1);
    assert_eq!(SCHEDULE.load(), 0);
    assert_eq!(DROP_F.load(), 0);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    w.wake();
    chan.recv().unwrap().run();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 0);
    assert_eq!(chan.len(), 0);

    waker().wake();
    assert_eq!(POLL.load(), 2);
    assert_eq!(SCHEDULE.load(), 1);
    assert_eq!(DROP_F.load(), 1);
    assert_eq!(DROP_S.load(), 1);
    assert_eq!(chan.len(), 0);
}
