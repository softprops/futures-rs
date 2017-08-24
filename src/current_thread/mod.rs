
#![allow(warnings, missing_docs)]

use std::prelude::v1::*;

use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use {Async, Poll};
use executor::{spawn, Spawn, NotifyHandle, Notify, enter};
use future::{Future, Executor, ExecuteError};
use stream::FuturesUnordered;

mod sink;
mod stream;

pub use self::sink::BlockingSink;
pub use self::stream::BlockingStream;

pub fn block_until<F: Future>(f: F) -> Result<F::Item, F::Error> {
    let mut future = spawn(f);
    let unpark = Arc::new(ThreadUnpark::new(thread::current()));
    let _e = enter();
    loop {
        match future.poll_future_notify(&unpark, 0)? {
            Async::NotReady => unpark.park(),
            Async::Ready(e) => return Ok(e),
        }
    }
}

pub fn block_on_all<F>(f: F)
	where F: FnOnce(&Spawner)
{
    let mut tasks = TaskRunner::new();
    f(&tasks.spawner());
    tasks.block_on_all();
}

// An object for cooperatively executing multiple tasks on a single thread.
// Useful for working with non-`Send` futures.
//
// NB: this is not `Send`
pub struct TaskRunner {
    inner: Rc<Inner>,
    non_daemons: usize,
    futures: Spawn<FuturesUnordered<SpawnedFuture>>,
}

struct Inner {
    new_futures: RefCell<Vec<(Box<Future<Item=(), Error=()>>, bool)>>,
}

impl TaskRunner {
	pub fn new() -> TaskRunner {
        TaskRunner {
            non_daemons: 0,
            futures: spawn(FuturesUnordered::new()),
            inner: Rc::new(Inner {
                new_futures: RefCell::new(Vec::new()),
            })
        }
    }

	pub fn spawner(&self) -> Spawner {
        Spawner { inner: Rc::downgrade(&self.inner) }
    }

	pub fn block_on_all(&mut self) {
        let unpark = Arc::new(ThreadUnpark::new(thread::current()));
        let _e = enter();
        loop {
            self.poll(&unpark);
            if self.non_daemons == 0 {
                break
            }
            unpark.park();
        }
    }

	pub fn block_until<F: Future>(&mut self, f: F) -> Result<F::Item, F::Error> {
        let unpark = Arc::new(ThreadUnpark::new(thread::current()));
        let _e = enter();
        let mut future = spawn(f);
        loop {
            if let Async::Ready(e) = future.poll_future_notify(&unpark, 0)? {
                return Ok(e)
            }
            self.poll(&unpark);
            unpark.park();
        }
    }

    fn poll(&mut self, unpark: &Arc<ThreadUnpark>) {
        loop {
            // Make progress on all spawned futures as long as we can
            loop {
                match self.futures.poll_stream_notify(unpark, 0) {
                    // If a daemon exits, then we ignore it, but if a non-daemon
                    // exits then we update our counter of non-daemons
                    Ok(Async::Ready(Some(daemon))) |
                    Err(daemon) => {
                        if !daemon {
                            self.non_daemons -= 1;
                        }
                    }
                    Ok(Async::NotReady) |
                    Ok(Async::Ready(None)) => break,
                }
            }

            // Now that we've made as much progress as we can, check our list of
            // spawned futures to see if anything was spawned
            let mut futures = self.inner.new_futures.borrow_mut();
            if futures.len() == 0 {
                break
            }
            for (future, daemon) in futures.drain(..) {
                if !daemon {
                    self.non_daemons += 1;
                }
                self.futures.get_mut().push(SpawnedFuture {
                    daemon: daemon,
                    inner: future,
                });
            }
        }
    }
}

impl fmt::Debug for TaskRunner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TaskRunner").finish()
    }
}

struct SpawnedFuture {
    daemon: bool,
    inner: Box<Future<Item = (), Error = ()>>,
}

impl Future for SpawnedFuture {
    type Item = bool;
    type Error = bool;

    fn poll(&mut self) -> Poll<bool, bool> {
        match self.inner.poll() {
            Ok(e) => Ok(e.map(|()| self.daemon)),
            Err(()) => Err(self.daemon),
        }
    }
}

#[derive(Clone)]
pub struct Spawner {
    inner: Weak<Inner>,
}

impl Spawner {
	pub fn spawn<F>(&self, task: F)
		where F: Future<Item = (), Error = ()> + 'static
    {
        self._spawn(Box::new(task), false);
    }

	pub fn spawn_daemon<F>(&self, task: F)
		where F: Future<Item = (), Error = ()> + 'static
    {
        self._spawn(Box::new(task), true);
    }

    fn _spawn(&self,
              future: Box<Future<Item = (), Error = ()>>,
              daemon: bool) {
        let inner = match self.inner.upgrade() {
            Some(i) => i,
            None => return,
        };
        inner.new_futures.borrow_mut().push((future, daemon));
    }
}

impl<F> Executor<F> for Spawner
	where F: Future<Item = (), Error = ()> + 'static
{
    fn execute(&self, future: F) -> Result<(), ExecuteError<F>> {
        self.spawn(future);
        Ok(())
    }
}

impl fmt::Debug for Spawner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Spawner").finish()
    }
}

struct ThreadUnpark {
    thread: thread::Thread,
    ready: AtomicBool,
}

impl ThreadUnpark {
    fn new(thread: thread::Thread) -> ThreadUnpark {
        ThreadUnpark {
            thread: thread,
            ready: AtomicBool::new(false),
        }
    }

    fn park(&self) {
        if !self.ready.swap(false, Ordering::SeqCst) {
            thread::park();
        }
    }
}

impl Notify for ThreadUnpark {
    fn notify(&self, _unpark_id: usize) {
        self.ready.store(true, Ordering::SeqCst);
        self.thread.unpark()
    }
}

