use core::cell::UnsafeCell;
use core::ops::Deref;
use core::sync::atomic::Ordering;
use std::sync::Arc;
use core::{marker::PhantomData, sync::atomic::AtomicU8};

use crate::{Ptr, Shared};

/// Data won't change while holding the Guard. This also means the Writer can only issue one swap, while Guard is being held
#[derive(Debug)]
pub struct ReadGuard<'a, T> {
    data: &'a UnsafeCell<T>,
    state: &'a AtomicU8,
    // PhantomData makes the borrow checker prove that there only ever is one ReadGuard
    // This is needed because on Drop the ReadGuard sets current_read to None
    reader: PhantomData<&'a mut Reader<T>>,
}

impl<'a, T> Deref for ReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.data.get() }
    }
}

impl<T, E> AsRef<E> for ReadGuard<'_, T>
where
    E: ?Sized,
    <Self as Deref>::Target: AsRef<E>,
{
    fn as_ref(&self) -> &E {
        self.deref().as_ref()
    }
}

impl<T> Drop for ReadGuard<'_, T> {
    fn drop(&mut self) {
        // release the read lock
        self.state.fetch_and(0b100, Ordering::Release);
    }
}

/// Dropping the Reader isn't realtime safe, because if dropped after the Writer, it deallocates.
/// Should only get dropped, when closing the real-time thread
///
/// Reader will be able to read data even if Writer has been dropped. Obviously that data won't change anymore
/// When there is no Reader the Writer is able to create a new one. The other way around doesn't work
pub struct Reader<T> {
    pub(crate) inner: Arc<Shared<T>>,
}

impl<T> Reader<T> {
    /// this function never blocks
    pub fn lock(&mut self) -> ReadGuard<'_, T> {
        // sets the corresponding read bit to the write ptr bit
        // happens as a single atomic operation so the 'double read' state isn't needed
        // ptr bit doesnt get changed
        let ptr = unsafe {
            self.inner
                .state
                .fetch_update(Ordering::Relaxed, Ordering::Acquire, |value| {
                    match value.into() {
                        Ptr::Value1 => Some(0b001),
                        Ptr::Value2 => Some(0b110),
                    }
                })
                .unwrap_unchecked()
                .into()
        };

        let data = self.inner.get_value(ptr);

        ReadGuard {
            data,
            state: &self.inner.state,
            reader: PhantomData,
        }
    }
}
