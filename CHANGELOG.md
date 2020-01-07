# Version 1.2.0

- The crate is now marked with `#![no_std]`.
- Add `Task::waker` and `JoinHandle::waker`.
- Add `Task::into_raw` and `Task::from_raw`.

# Version 1.1.1

- Fix a use-after-free bug where the schedule function is dropped while running.

# Version 1.1.0

- If a task is dropped or cancelled outside the `run` method, it gets re-scheduled.
- Add `spawn_local` constructor.

# Version 1.0.0

- Initial release
