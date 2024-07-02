use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use atomic_wait::{wait, wake_one};

pub struct MutexGuard<'a, T> {
    inner: &'a Mutex<T>,
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        if self.inner.state.swap(0, Ordering::Release) == 2 {
            // Wake a single waiting thread, if any
            wake_one(&self.inner.state);
        }
    }
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.inner.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.inner.value.get() }
    }
}

pub struct Mutex<T> {
    // 0: unlocked
    // 1: locked with waiting threads
    // 2: locked with no waiting threads
    // This optimisation avoids unnecessary syscalls for waking waiting threads
    // by tracking when a wake is actually required.
    state: AtomicU32,
    value: UnsafeCell<T>,
}

unsafe impl<T> Sync for Mutex<T> where T: Send {}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> MutexGuard<T> {
        // If an err occurs on the swap, the mutex has been locked previously
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            Self::lock_contended(&self.state);
        }
        MutexGuard { inner: self }
    }

    fn lock_contended(state: &AtomicU32) {
        let mut spin_count = 0;
        // Basic spinlock behaviour to avoid sleeping the thread on very short
        // lock hold durations. This is much more efficient than the subsequent
        // syscalls for the wait/wake case.
        // Note that we only do this when the lock has no waiters.
        while state.load(Ordering::Relaxed) == 1 && spin_count < 100 {
            spin_count += 1;
            std::hint::spin_loop();
        }

        // Lock the mutex with no waiters state if it is unlocked and return
        if state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }

        // Wait the thread when not unlocked after failing to unlock earlier
        // with our hybrid spinlock impl
        while state.swap(2, Ordering::Acquire) != 0 {
            wait(state, 2);
        }
    }
}
