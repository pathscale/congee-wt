use std::{marker::PhantomData, ptr::NonNull, sync::atomic::Ordering};

#[cfg(all(feature = "shuttle", test))]
use shuttle::sync::atomic::fence;

#[cfg(not(all(feature = "shuttle", test)))]
use std::sync::atomic::fence;

use crate::{
    error::ArtError,
    nodes::{BaseNode, Node},
};

pub(crate) struct TypedReadGuard<'a, T: Node> {
    version: u32,
    node: *const T,
    _pt_node: PhantomData<&'a T>,
}

impl<'a, T: Node> TypedReadGuard<'a, T> {
    pub(crate) fn as_ref(&self) -> &T {
        unsafe { &*self.node }
    }

    pub(crate) fn upgrade(self) -> Result<TypedWriteGuard<'a, T>, (Self, ArtError)> {
        let new_version = self.version + 0b10;
        match self
            .as_ref()
            .base()
            .version_lock_obsolete
            .compare_exchange_weak(
                self.version,
                new_version,
                // AcqRel: lock acquisition needs Acquire so the critical section
                // cannot be reordered before taking the lock; Release publishes
                // the version bump to readers.
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
            Ok(_) => Ok(TypedWriteGuard {
                node: unsafe { &mut *(self.node as *mut T) },
            }),
            Err(_v) => {
                #[cfg(all(feature = "shuttle", test))]
                shuttle::thread::yield_now();
                Err((self, ArtError::VersionNotMatch))
            }
        }
    }
}

pub(crate) struct TypedWriteGuard<'a, T: Node> {
    node: &'a mut T,
}

impl<T: Node> TypedWriteGuard<'_, T> {
    pub(crate) fn as_ref(&self) -> &T {
        self.node
    }

    pub(crate) fn as_mut(&mut self) -> &mut T {
        self.node
    }

    pub(crate) fn mark_obsolete(&self) {
        self.node
            .base()
            .version_lock_obsolete
            .fetch_add(0b01, Ordering::Release);
    }
}

impl<T: Node> Drop for TypedWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.node
            .base()
            .version_lock_obsolete
            .fetch_add(0b10, Ordering::Release);
    }
}

pub(crate) struct ReadGuard<'a> {
    version: u32,
    node: NonNull<BaseNode>,
    _pt_node: PhantomData<&'a BaseNode>,
}

impl<'a> ReadGuard<'a> {
    pub(crate) fn new(v: u32, node: NonNull<BaseNode>) -> Self {
        Self {
            version: v,
            node,
            _pt_node: PhantomData,
        }
    }

    pub(crate) fn check_version(&self) -> Result<u32, ArtError> {
        // Seqlock reader pattern (as in crossbeam's AtomicCell validate_read):
        // the Acquire fence keeps the preceding data loads from sinking below
        // the re-validation load, so the load itself carries no ordering duty
        // and can be Relaxed; nothing after it depends on it for ordering.
        fence(Ordering::Acquire);
        let v = self.as_ref().version_lock_obsolete.load(Ordering::Relaxed);

        if v == self.version {
            Ok(v)
        } else {
            #[cfg(all(feature = "shuttle", test))]
            shuttle::thread::yield_now();
            Err(ArtError::VersionNotMatch)
        }
    }

    pub(crate) fn unlock(self) -> Result<u32, ArtError> {
        self.check_version()
    }

    #[must_use]
    pub(crate) fn into_typed<T: Node>(self) -> TypedReadGuard<'a, T> {
        assert_eq!(self.as_ref().get_type(), T::get_type());

        TypedReadGuard {
            version: self.version,
            node: unsafe { &*(self.node.as_ptr() as *const T) },
            _pt_node: PhantomData,
        }
    }

    pub(crate) fn as_ref(&self) -> &BaseNode {
        unsafe { &*self.node.as_ptr() }
    }

    pub(crate) fn upgrade(self) -> Result<WriteGuard<'a>, (Self, ArtError)> {
        let new_version = self.version + 0b10;
        match self.as_ref().version_lock_obsolete.compare_exchange_weak(
            self.version,
            new_version,
            // AcqRel: lock acquisition needs Acquire so the critical section
            // cannot be reordered before taking the lock; Release publishes
            // the version bump to readers.
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(WriteGuard {
                node: unsafe { &mut *(self.node.as_ptr()) },
            }),
            Err(_v) => {
                #[cfg(all(feature = "shuttle", test))]
                shuttle::thread::yield_now();
                Err((self, ArtError::VersionNotMatch))
            }
        }
    }
}

pub(crate) struct WriteGuard<'a> {
    node: &'a mut BaseNode,
}

impl WriteGuard<'_> {
    pub(crate) fn as_ref(&self) -> &BaseNode {
        self.node
    }

    pub(crate) fn as_mut(&mut self) -> &mut BaseNode {
        self.node
    }

    pub(crate) fn mark_obsolete(&mut self) {
        self.node
            .version_lock_obsolete
            .fetch_add(0b01, Ordering::Release);
    }
}

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        self.node
            .version_lock_obsolete
            .fetch_add(0b10, Ordering::Release);
    }
}
