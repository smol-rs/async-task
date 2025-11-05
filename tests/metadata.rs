use async_task::{Builder, Runnable};
use flume::unbounded;
use smol::future;

use std::{
    ptr::NonNull,
    sync::atomic::{AtomicUsize, Ordering},
};

#[test]
fn metadata_use_case() {
    // Each future has a counter that is incremented every time it is scheduled.
    let (sender, receiver) = unbounded::<Runnable<AtomicUsize>>();
    let schedule = move |runnable: Runnable<AtomicUsize>| {
        runnable.metadata().fetch_add(1, Ordering::SeqCst);
        sender.send(runnable).ok();
    };

    async fn my_future(counter: &AtomicUsize) {
        loop {
            // Loop until we've been scheduled five times.
            let count = counter.load(Ordering::SeqCst);
            if count < 5 {
                // Make sure that we are immediately scheduled again.
                future::yield_now().await;
                continue;
            }

            // We've been scheduled five times, so we're done.
            break;
        }
    }

    let make_task = || {
        // SAFETY: We are spawning a non-'static future, so we need to use the unsafe API.
        // The borrowed variables, in this case the metadata, are guaranteed to outlive the runnable.
        let (runnable, task) = unsafe {
            Builder::new()
                .metadata(AtomicUsize::new(0))
                .spawn_unchecked(my_future, schedule.clone())
        };

        runnable.schedule();
        task
    };

    // Make tasks.
    let t1 = make_task();
    let t2 = make_task();

    // Run the tasks.
    while let Ok(runnable) = receiver.try_recv() {
        runnable.run();
    }

    // Unwrap the tasks.
    smol::future::block_on(async move {
        t1.await;
        t2.await;
    });
}

#[test]
fn metadata_raw() {
    let (sender, receiver) = unbounded::<NonNull<()>>();

    let future = |counter: &AtomicUsize| {
        assert_eq!(0, counter.fetch_add(1, Ordering::SeqCst));
        async {}
    };

    let schedule = move |runnable: Runnable<AtomicUsize>| {
        let ptr = runnable.into_raw();

        {
            let counter: &AtomicUsize = unsafe { Runnable::metadata_raw(ptr).as_ref() };
            assert_eq!(1, counter.fetch_add(1, Ordering::SeqCst));
        }

        sender.send(ptr).ok();
    };

    let (runnable, task) = unsafe {
        Builder::new()
            .metadata(AtomicUsize::new(0))
            .spawn_unchecked(future, schedule)
    };

    runnable.schedule();
    let ptr = receiver.recv().unwrap();

    {
        let counter: &AtomicUsize = unsafe { Runnable::metadata_raw(ptr).as_ref() };
        assert_eq!(2, counter.fetch_add(1, Ordering::SeqCst));
    }

    let runnable: Runnable<AtomicUsize> = unsafe { Runnable::from_raw(ptr) };
    assert_eq!(3, runnable.metadata().fetch_add(1, Ordering::SeqCst));

    assert_eq!(4, task.metadata().load(Ordering::SeqCst));
}
