use std::pin::pin;

use smol::future;

#[test]
fn cancelled_after_task_drop() {
    let f = async {};
    let (runnable, task) = async_task::spawn(f, |_| {});
    assert!(!runnable.is_cancelled());
    drop(task);
    assert!(runnable.is_cancelled());
}

#[test]
fn cancelled_after_task_cancel() {
    let f = async {};
    let (runnable, task) = async_task::spawn(f, |_| {});

    assert!(!runnable.is_cancelled());

    let mut cancel_fut = pin!(task.cancel());
    assert!(future::block_on(future::poll_once(&mut cancel_fut)).is_none());
    assert!(runnable.is_cancelled());
}
