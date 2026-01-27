//! Implements task-local variables.

use std::any::Any;
use std::boxed::Box;
use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};
use std::thread_local;

/// Macro for defining task local variables.
#[macro_export]
macro_rules! task_local {
    // Terminator case.
    () => {};

    ($(#[$attr:meta])* $vis:vis static $name:ident : $t:ty = $init:expr) => {
        $(#[$attr])* $vis static $name: $crate::TaskLocalKey<$t> = {
            #[inline]
            fn __init() -> $t {
                $init
            }

            $crate::TaskLocalKey::new(__init)
        };
    };

    ($(#[$attr:meta])* $vis:vis static $name:ident : $t:ty = $init:expr; $($rest:tt)*) => {
        $crate::task_local!($(#[$attr])* $vis static $name: $t = $init);
        $crate::task_local!($($rest)*);
    }
}

/// A key indexing a task-local variable.
pub struct TaskLocalKey<T> {
    /// Key for the task local.
    key: AtomicU32,

    /// Initializer function.
    init: fn() -> T,
}

impl<T: Send + 'static> TaskLocalKey<T> {
    /// Hidden constructor for [`TaskLocalKey`].
    #[doc(hidden)]
    #[inline]
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            key: AtomicU32::new(0),
            init,
        }
    }

    /// Run a closure with the value.
    ///
    /// The closure will not run reentrantly, even across nested tasks.
    #[inline]
    pub fn try_with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        LocalMap::with(|map| {
            let value = map.get_or_insert(self.key(), || Box::new((self.init)()));

            // SAFETY: Under this key, `try_with` is guaranteed to be this type.
            let value = unsafe { &*(value as *const (dyn Any + Send) as *const T) };

            f(value)
        })
    }

    /// Get the key for the map.
    #[inline]
    fn key(&self) -> u32 {
        let key = self.key.load(Ordering::Relaxed);
        if key != 0 {
            return key;
        }

        self.key_cold()
    }

    /// Cold branch of [`key`].
    #[cold]
    fn key_cold(&self) -> u32 {
        static COUNTER: AtomicU32 = AtomicU32::new(1);

        let key = COUNTER.fetch_add(1, Ordering::Relaxed);
        if key > u32::MAX / 2 {
            crate::utils::abort();
        }

        // Store the key back in.
        match self
            .key
            .compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => key,
            Err(k) => k,
        }
    }
}

/// Change a future into a future that uses task-local variables.
#[inline]
pub(crate) fn task_local<F: Future>(fut: F) -> impl Future<Output = F::Output> {
    /// The underlying task-local future.
    struct TaskLocal<F> {
        /// Inner future.
        future: F,

        /// Map of locals, pinned to the stack.
        ///
        /// ## Safety
        ///
        /// This is stored in a thread-local variable. Only the running future should
        /// be able to access it.
        locals: UnsafeCell<LocalMap>,

        /// Pinned to the stack.
        _pin: PhantomPinned,
    }

    impl<F: Future> Future for TaskLocal<F> {
        type Output = F::Output;

        #[inline]
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            unsafe {
                let local = NonNull::new_unchecked(self.locals.get());
                LocalMap::set(local, move || {
                    Pin::new_unchecked(&mut self.get_unchecked_mut().future).poll(cx)
                })
            }
        }
    }

    TaskLocal {
        future: fut,
        locals: UnsafeCell::new(LocalMap::new()),
        _pin: PhantomPinned,
    }
}

/// Map matching thread local keys to values.
struct LocalMap {
    /// Inner map.
    inner: Option<HashMap<u32, Box<dyn Any + Send>>>,
}

impl LocalMap {
    /// Create a new map of locals.
    #[inline]
    fn new() -> Self {
        Self { inner: None }
    }

    /// Get the current [`LocalMap`] for the task, and do something with it.
    ///
    /// Returns `None` if a [`LocalMap`] is not set or if we would interfere
    /// with another `with` call.
    #[inline]
    fn with<R>(f: impl FnOnce(&mut LocalMap) -> R) -> Option<R> {
        /// State of the global [`LocalMap`].
        struct GlobalState {
            /// Current local map, or `None` if there is none.
            current: Cell<Option<NonNull<LocalMap>>>,

            /// Set to `true` when we are inside of `with()`.
            ///
            /// This prevents recursive calls to `set()` and `with()` where we try
            /// polling tasks inside of eachother.
            in_use: Cell<bool>,
        }

        thread_local! {
            /// Global state.
            static STATE: GlobalState = const {
                GlobalState {
                    current: Cell::new(None),
                    in_use: Cell::new(false)
                }
            };
        }

        impl LocalMap {
            /// Set the [`LocalMap`] in place.
            ///
            /// ## Safety
            ///
            /// Must be a valid mutable pointer to a [`LocalMap`].
            ///
            /// ## Panics
            ///
            /// Panics if called inside of `with`.
            #[inline]
            pub(super) unsafe fn set<R>(map: NonNull<LocalMap>, f: impl FnOnce() -> R) -> R {
                STATE.with(|state| {
                    if state.in_use.get() {
                        panic!("tried to access a task-local variable inside of LocalKey::with()");
                    }

                    let prev = state.current.replace(Some(map));
                    let _guard = CallOnDrop(|| state.current.set(prev));
                    f()
                })
            }
        }

        STATE
            .try_with(move |state| {
                // If there is no current thread state, return None.
                let mut current = match state.current.get() {
                    Some(current) => current,
                    None => return None,
                };

                // Mark us as being in use.
                if state.in_use.replace(true) {
                    return None;
                }
                let _lock = CallOnDrop(|| state.in_use.set(false));

                // SAFETY: `current` is guaranteed to be a valid pointer to a `LocalMap`.
                Some(f(unsafe { current.as_mut() }))
            })
            .ok()
            .flatten()
    }

    /// Get a value by its index, or insert a new one.
    #[inline]
    fn get_or_insert(
        &mut self,
        key: u32,
        or_else: impl FnOnce() -> Box<dyn Any + Send>,
    ) -> &(dyn Any + Send) {
        use std::collections::hash_map::Entry;

        match self.inner.get_or_insert_with(|| HashMap::new()).entry(key) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => &*v.insert(or_else()),
        }
    }
}

struct CallOnDrop<F: FnMut()>(F);
impl<F: FnMut()> Drop for CallOnDrop<F> {
    #[inline]
    fn drop(&mut self) {
        (self.0)();
    }
}
