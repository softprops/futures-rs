//! Executors
//!
//! This module contains tools for managing the raw execution of futures,
//! which is needed when building *executors* (places where futures can run).
//!
//! More information about executors can be [found online at tokio.rs][online].
//!
//! [online]: https://tokio.rs/docs/going-deeper-futures/tasks/

#[allow(deprecated)]

pub use task_impl::{Spawn, spawn, Notify, with_notify};
pub use task_impl::{UnsafeNotify, NotifyHandle};

#[cfg(feature = "use_std")]
pub use self::std_support::*;

#[cfg(feature = "use_std")]
#[allow(missing_docs)]
mod std_support {
    use std::cell::Cell;

    #[allow(deprecated)]
    pub use task_impl::{Unpark, Executor, Run};

    thread_local!(static ENTERED: Cell<bool> = Cell::new(false));

    #[derive(Debug)]
    pub struct Enter {
        _priv: (),
    }

    pub fn enter() -> Enter {
        ENTERED.with(|c| {
            if c.get() {
                panic!("cannot reenter an executor context");
            }
            c.set(true);
        });

        Enter { _priv: () }
    }

    impl Drop for Enter {
        fn drop(&mut self) {
            ENTERED.with(|c| c.set(false));
        }
    }
}
