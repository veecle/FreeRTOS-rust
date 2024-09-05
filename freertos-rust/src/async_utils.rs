//! Free functions for internal async usage.

use core::task::Waker;

/// This function should register a waker to be woken on queue event. This is just a prototype
/// and the end result may be very different.
///
/// For now this directly wakers the waker.
#[inline]
pub fn register_queue_waker(_queue_id: u32, waker: &Waker) {
    waker.wake_by_ref()
}
