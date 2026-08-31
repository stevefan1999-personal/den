use std::{
    mem::{align_of, size_of},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use rquickjs::allocator::{Allocator, RustAllocator};

const ALLOC_ALIGN: usize = align_of::<u64>();
const HEADER_SIZE: usize = if size_of::<usize>() > ALLOC_ALIGN {
    size_of::<usize>()
} else {
    ALLOC_ALIGN
};

/// QuickJS allocator backed by Rust's global allocator with an optional hard
/// limit on live usable bytes.
pub(super) struct BoundedAllocator {
    inner: RustAllocator,
    limit: Option<usize>,
    used:  AtomicUsize,
}

impl BoundedAllocator {
    pub(super) const fn new(limit: Option<usize>) -> Self {
        Self {
            inner: RustAllocator,
            limit,
            used: AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    pub(super) fn used(&self) -> usize { self.used.load(Ordering::Acquire) }

    /// RustAllocator rounds every usable allocation to `u64` alignment and
    /// prefixes it with one aligned `usize` header. Validate both operations
    /// before calling it: its internal arithmetic assumes they cannot overflow.
    fn usable_size_for(size: usize) -> Option<usize> {
        let rounded = size.checked_add(ALLOC_ALIGN - 1)? & !(ALLOC_ALIGN - 1);
        rounded.checked_add(HEADER_SIZE).map(|_| rounded)
    }

    fn reserve(&self, size: usize) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                let next = used.checked_add(size)?;
                self.limit.is_none_or(|limit| next <= limit).then_some(next)
            })
            .is_ok()
    }

    fn release(&self, size: usize) {
        let released = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_sub(size)
            });
        debug_assert!(
            released.is_ok(),
            "allocator accounting cannot release more bytes than are live"
        );
    }
}

// SAFETY: every pointer is allocated, resized, measured and freed by the same
// RustAllocator. Reservations use its exact rounding rule and are rolled back
// whenever the underlying operation fails.
unsafe impl Allocator for BoundedAllocator {
    fn alloc(&mut self, size: usize) -> *mut u8 {
        let Some(accounted) = Self::usable_size_for(size) else {
            return ptr::null_mut();
        };
        if !self.reserve(accounted) {
            return ptr::null_mut();
        }

        let allocation = self.inner.alloc(size);
        if allocation.is_null() {
            self.release(accounted);
        }
        allocation
    }

    fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
        let Some(total) = count.checked_mul(size) else {
            return ptr::null_mut();
        };
        if total == 0 {
            return ptr::null_mut();
        }
        let Some(accounted) = Self::usable_size_for(total) else {
            return ptr::null_mut();
        };
        if !self.reserve(accounted) {
            return ptr::null_mut();
        }

        let allocation = self.inner.calloc(count, size);
        if allocation.is_null() {
            self.release(accounted);
        }
        allocation
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        // SAFETY: the Allocator contract guarantees this pointer came from us;
        // we forward it to the same RustAllocator that created it.
        let size = unsafe { RustAllocator::usable_size(ptr) };
        self.release(size);
        unsafe { self.inner.dealloc(ptr) };
    }

    unsafe fn realloc(&mut self, ptr: *mut u8, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(new_size);
        }

        let Some(accounted) = Self::usable_size_for(new_size) else {
            return ptr::null_mut();
        };
        // SAFETY: the Allocator contract guarantees this pointer came from us.
        let old_size = unsafe { RustAllocator::usable_size(ptr) };
        let growth = accounted.saturating_sub(old_size);
        if !self.reserve(growth) {
            return ptr::null_mut();
        }

        // SAFETY: the pointer came from `inner`; a null result leaves the old
        // allocation live, matching realloc's contract.
        let resized = unsafe { self.inner.realloc(ptr, new_size) };
        if resized.is_null() {
            self.release(growth);
        } else if old_size > accounted {
            self.release(old_size - accounted);
        }
        resized
    }

    unsafe fn usable_size(ptr: *mut u8) -> usize
    where
        Self: Sized,
    {
        // SAFETY: callers must supply a pointer allocated by this allocator,
        // which is exactly a RustAllocator pointer.
        unsafe { RustAllocator::usable_size(ptr) }
    }
}
