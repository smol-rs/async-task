use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn wake() {
    let a = Arc::new(AtomicUsize::new(0));
    let w = async_task::waker_fn({
        let a = a.clone();
        move || {
            a.fetch_add(1, Ordering::SeqCst);
        }
    });

    assert_eq!(a.load(Ordering::SeqCst), 0);
    w.wake_by_ref();
    assert_eq!(a.load(Ordering::SeqCst), 1);

    let w2 = w.clone();
    assert_eq!(a.load(Ordering::SeqCst), 1);
    w2.wake_by_ref();
    assert_eq!(a.load(Ordering::SeqCst), 2);
    drop(w2);
    assert_eq!(a.load(Ordering::SeqCst), 2);

    let w3 = w.clone();
    assert_eq!(a.load(Ordering::SeqCst), 2);
    w3.wake();
    assert_eq!(a.load(Ordering::SeqCst), 3);
}
