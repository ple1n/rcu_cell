use alloc::sync::Arc;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::ptr;
use core::sync::atomic::Ordering;

use crate::link::LinkWrapper;
use crate::ArcPointer;

#[inline]
fn ptr_to_arc<T>(ptr: *const T) -> Arc<T> {
    unsafe { ArcPointer::from_raw(ptr) }
}

/// RCU cell that never contains None, behaves like `RwLock<Arc<T>>`
#[derive(Debug)]
pub struct RcuCellNonNull<T> {
    link: LinkWrapper<T>,
}

unsafe impl<T: Send> Send for RcuCellNonNull<T> {}
unsafe impl<T: Send + Sync> Sync for RcuCellNonNull<T> {}

impl<T> Drop for RcuCellNonNull<T> {
    fn drop(&mut self) {
        let ptr = self.link.get_ref();
        let _ = ptr_to_arc(ptr);
    }
}

impl<T: Default> Default for RcuCellNonNull<T> {
    fn default() -> Self {
        RcuCellNonNull::new(Default::default())
    }
}

impl<T> From<Arc<T>> for RcuCellNonNull<T> {
    fn from(data: Arc<T>) -> Self {
        let arc_ptr = Arc::into_raw(data);
        RcuCellNonNull {
            link: LinkWrapper::new(arc_ptr),
        }
    }
}

impl<T> RcuCellNonNull<T> {
    /// create rcu cell from a value
    #[inline]
    pub fn new(data: T) -> Self {
        let ptr = Arc::into_raw(Arc::new(data));
        RcuCellNonNull {
            link: LinkWrapper::new(ptr),
        }
    }

    /// convert the rcu cell to an Arc value
    #[inline]
    pub fn into_arc(self) -> Arc<T> {
        let ptr = self.link.get_ref();
        let ret = ptr_to_arc(ptr);
        let _ = ManuallyDrop::new(self);
        ret
    }

    /// write a value to the rcu cell and return the old value
    #[inline]
    pub fn write(&self, data: impl Into<Arc<T>>) -> Arc<T> {
        let data = data.into();
        let new_ptr = Arc::into_raw(data);
        ptr_to_arc(self.link.update(new_ptr))
    }

    /// Atomicly update the value with a closure and return the old value.
    /// The closure will be called with the old value and return the new value.
    pub fn update<R, F>(&self, f: F) -> Arc<T>
    where
        F: FnOnce(Arc<T>) -> R,
        R: Into<Arc<T>>,
    {
        // increase ref count to lock the inner Arc
        let ptr = self.link.lock_read();
        let old_value = ptr_to_arc(ptr);
        let new_ptr = Arc::into_raw(f(old_value.clone()).into());
        self.link.unlock_update(new_ptr);
        old_value
    }

    /// read out the inner Arc value
    #[inline]
    pub fn read(&self) -> Arc<T> {
        let ptr = self.link.inc_ref();
        let v = ManuallyDrop::new(ptr_to_arc(ptr));
        let cloned = v.deref().clone();
        self.link.dec_ref();
        core::sync::atomic::fence(Ordering::Acquire);
        cloned
    }

    /// read inner ptr and check if it is the same as the given Arc
    #[inline]
    pub fn arc_eq(&self, data: &Arc<T>) -> bool {
        core::ptr::eq(self.link.get_ref(), Arc::as_ptr(data))
    }

    /// check if two RcuCellNonNull instances point to the same inner Arc
    #[inline]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        core::ptr::eq(this.link.get_ref(), other.link.get_ref())
    }
}

#[cfg(feature = "serde")]
mod ser {
    use super::*;
    use serde::{Deserialize, Serialize};

    impl<T: Serialize> Serialize for RcuCellNonNull<T> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            self.read().serialize(serializer)
        }
    }

    impl<'de, T: Deserialize<'de>> Deserialize<'de> for RcuCellNonNull<T> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let value = T::deserialize(deserializer)?;
            Ok(RcuCellNonNull::new(value))
        }
    }
}
