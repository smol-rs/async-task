#![feature(test)]

extern crate test;

use smol::future;
use test::Bencher;

#[bench]
fn task_create(b: &mut Bencher) {
    b.iter(|| {
        let _ = async_task::spawn(async {}, drop);
    });
}

#[bench]
fn task_run(b: &mut Bencher) {
    b.iter(|| {
        let (runnable, task) = async_task::spawn(async {}, drop);
        runnable.run();
        future::block_on(task);
    });
}

#[bench]
fn task_wake_during_poll(b: &mut Bencher) {
    b.iter(|| {
        let mut woken = false;
        let future = future::poll_fn(move |cx| {
            if woken {
                return std::task::Poll::Ready(());
            }
            woken = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        });
        let (runnable, task) = async_task::spawn(future, |r: async_task::Runnable| {
            r.run();
        });
        runnable.run();
        future::block_on(task);
    });
}

#[bench]
fn task_clone_waker_during_poll(b: &mut Bencher) {
    b.iter(|| {
        let mut waker = None;
        let future = future::poll_fn(move |cx| {
            if waker.is_some() {
                return std::task::Poll::Ready(());
            }
            waker = Some(cx.waker().clone());
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        });
        let (runnable, task) = async_task::spawn(future, |r: async_task::Runnable| {
            r.run();
        });
        runnable.run();
        future::block_on(task);
    });
}
