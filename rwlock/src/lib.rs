use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use atomic_wait::{wait, wake_all, wake_one};

pub struct WriteGuard<'a, T> {
    inner: &'a RwLock<T>,
}

impl<T> Drop for WriteGuard<'_, T> {
    fn drop(&mut self) {
        // Reset counter
        self.inner.state.store(0, Ordering::Release);
        // We don't know whether readers/writers are waiting, so wake all threads
        // and allow the internal logic to handle the rest.
        wake_all(&self.inner.state);
    }
}

impl<T> Deref for WriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.inner.value.get() }
    }
}

// DerefMut is implemented for WriteGuard because it requires exclusive access
// to the data. The same is NOT true for the ReadGuard
impl<T> DerefMut for WriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.inner.value.get() }
    }
}

pub struct ReadGuard<'a, T> {
    inner: &'a RwLock<T>,
}

impl<T> Deref for ReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.inner.value.get() }
    }
}

impl<T> Drop for ReadGuard<'_, T> {
    fn drop(&mut self) {
        // fetch_sub here means that if the returned value is 1, there are now
        // no more reader locks held. Wake up a slept thread to proceed.
        // This is because it returns what the PRIOR value was, before subtraction
        if self.inner.state.fetch_sub(1, Ordering::Release) == 1 {
            wake_one(&self.inner.state);
        }
    }
}

pub struct RwLock<T> {
    // Numbers of readers or `u32::MAX` when there is a writer lock
    state: AtomicU32,
    value: UnsafeCell<T>,
}

// We also include the "where Sync" here because multiple readers may have
// access to the underlying data and be shared across threads.
// NOTE: A writer is exclusive and does not have this requirement.
unsafe impl<T> Sync for RwLock<T> where T: Send + Sync {}

impl<T> RwLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub fn read(&self) -> ReadGuard<T> {
        let mut s = self.state.load(Ordering::Relaxed);
        loop {
            if s < u32::MAX {
                assert!(s != u32::MAX - 1, "too many readers");
                match self.state.compare_exchange_weak(
                    s,
                    s + 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return ReadGuard { inner: self },
                    Err(e) => s = e,
                }
            }

            // Write-locked, put the thread to sleep
            if s == u32::MAX {
                wait(&self.state, u32::MAX);
                s = self.state.load(Ordering::Relaxed);
            }
        }
    }

    pub fn write(&mut self) -> WriteGuard<T> {
        while let Err(s) =
            self.state
                .compare_exchange(0, u32::MAX, Ordering::Acquire, Ordering::Relaxed)
        {
            // Already write-locked, so wait
            wait(&self.state, s);
        }
        WriteGuard { inner: self }
    }
}
