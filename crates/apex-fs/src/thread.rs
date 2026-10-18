//! Threads in the browser, which has none: work given to a new thread is
//! done right away on this one, and its handle holds the result. Code
//! that waits for a thread's answer gets it at once; a thread meant to
//! run for the life of the program must not be started there.

use std::io;
use std::marker::PhantomData;
use std::time::Duration;

pub use std::thread::{Result, Thread, ThreadId, current, panicking, yield_now};

pub struct JoinHandle<T> {
    result: Option<T>,
}

impl<T> JoinHandle<T> {
    pub fn join(mut self) -> Result<T> {
        Ok(self.result.take().expect("joined once"))
    }

    pub fn is_finished(&self) -> bool {
        true
    }
}

impl<T> std::fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JoinHandle")
    }
}

pub fn spawn<F: FnOnce() -> T, T>(f: F) -> JoinHandle<T> {
    JoinHandle { result: Some(f()) }
}

/// Waiting would stop the page: the browser's frames go on instead.
pub fn sleep(_d: Duration) {}

pub fn available_parallelism() -> io::Result<std::num::NonZeroUsize> {
    Ok(std::num::NonZeroUsize::MIN)
}

#[derive(Debug, Default)]
pub struct Builder;

impl Builder {
    pub fn new() -> Builder {
        Builder
    }

    pub fn name(self, _name: String) -> Builder {
        self
    }

    pub fn stack_size(self, _size: usize) -> Builder {
        self
    }

    pub fn spawn<F: FnOnce() -> T, T>(self, f: F) -> io::Result<JoinHandle<T>> {
        Ok(spawn(f))
    }

    pub fn spawn_scoped<'scope, 'env, F: FnOnce() -> T + 'scope, T: 'scope>(self, s: &'scope Scope<'scope, 'env>, f: F) -> io::Result<ScopedJoinHandle<'scope, T>> {
        Ok(s.spawn(f))
    }
}

pub struct Scope<'scope, 'env: 'scope> {
    scope: PhantomData<&'scope mut &'scope ()>,
    env: PhantomData<&'env mut &'env ()>,
}

pub struct ScopedJoinHandle<'scope, T> {
    result: Option<T>,
    scope: PhantomData<&'scope ()>,
}

impl<T> ScopedJoinHandle<'_, T> {
    pub fn join(mut self) -> Result<T> {
        Ok(self.result.take().expect("joined once"))
    }

    pub fn is_finished(&self) -> bool {
        true
    }
}

impl<'scope> Scope<'scope, '_> {
    pub fn spawn<F: FnOnce() -> T + 'scope, T: 'scope>(&'scope self, f: F) -> ScopedJoinHandle<'scope, T> {
        ScopedJoinHandle { result: Some(f()), scope: PhantomData }
    }
}

pub fn scope<'env, F, T>(f: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    f(&Scope { scope: PhantomData, env: PhantomData })
}
