//! Rust wrappers around freeRTOS queues. Provide async compatibility.
use core::future::poll_fn;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::task::Poll;

use atomic_waker::AtomicWaker;

use crate::base::{
    freertos_rs_queue_create, freertos_rs_queue_delete, freertos_rs_queue_messages_waiting,
    freertos_rs_queue_receive, freertos_rs_queue_send, freertos_rs_queue_send_isr,
    FreeRtosMutVoidPtr, FreeRtosQueueHandle, FreeRtosVoidPtr,
};
use crate::isr::InterruptContext;
use crate::units::Duration;
use crate::FreeRtosError;

unsafe impl<T: Send + Sized> Send for Queue<T> {}
unsafe impl<T: Send + Sized> Sync for Queue<T> {}

/// A queue with a finite size. The items are owned by the queue and are
/// cloned on send.
///
/// ## Usage in FFIs
///
/// The implementation works with raw memory representations. This means
/// that the type `T` layout must be understandable by the receiver. This
/// is usually the case for types that are `Send` and `Sized` in Rust.
///
/// If communication with "C" is expected, users `must` ensure the types are
/// C-compatible. This can be achieved by annotating them with the `#[repr(C)]`
/// attribute.
#[derive(Debug)]
pub struct Queue<T: Send + Sized> {
    send_waker: AtomicWaker,
    receive_waker: AtomicWaker,
    queue: FreeRtosQueueHandle,
    item_type: PhantomData<T>,
}

impl<T: Send + Sized> Queue<T> {
    /// Create a new queue capable of holding `length` items of type `T`.
    pub fn new(length: usize) -> Result<Queue<T>, FreeRtosError> {
        let item_size = core::mem::size_of::<T>();
        let handle = unsafe { freertos_rs_queue_create(length as u32, item_size as u32) };

        if handle.is_null() {
            return Err(FreeRtosError::OutOfMemory);
        }

        Ok(Queue {
            send_waker: AtomicWaker::default(),
            receive_waker: AtomicWaker::default(),
            queue: handle,
            item_type: PhantomData,
        })
    }

    /// Return the number of messages waiting in the queue.
    #[inline]
    pub fn messages_waiting(&self) -> u32 {
        unsafe { freertos_rs_queue_messages_waiting(self.queue) }
    }

    /// Send an item to the end of the queue.
    ///
    /// Wait for the queue to have empty space for up to `max_wait`. If `max_size` is 0,
    /// the function will return immediately if the queue is full.
    #[inline]
    pub fn send_blocking(&self, item: &T, max_wait: Duration) -> Result<(), FreeRtosError> {
        let ptr = item as *const _ as FreeRtosVoidPtr;
        let ret = unsafe { freertos_rs_queue_send(self.queue, ptr, max_wait.ticks()) };

        if ret != 0 {
            Err(FreeRtosError::QueueSendTimeout)
        } else {
            self.receive_waker.wake();
            Ok(())
        }
    }

    /// Send an item to the end of the queue, from an interrupt.
    #[inline]
    pub fn send_isr(&self, context: &mut InterruptContext, item: &T) -> Result<(), FreeRtosError> {
        let ptr = item as *const _ as FreeRtosVoidPtr;
        let ret =
            unsafe { freertos_rs_queue_send_isr(self.queue, ptr, context.get_task_field_mut()) };

        if ret != 0 {
            Err(FreeRtosError::QueueFull)
        } else {
            self.receive_waker.wake();
            Ok(())
        }
    }

    /// Async version of `send_blocking`.
    ///
    /// The function will .await until the queue has space for the item.
    pub async fn send(&self, item: &T) {
        poll_fn(|cx| {
            let ptr = item as *const _ as FreeRtosVoidPtr;
            let ret = unsafe { freertos_rs_queue_send(self.queue, ptr, 0) };

            if ret == 0 {
                self.receive_waker.wake();
                Poll::Ready(())
            } else {
                self.send_waker.register(cx.waker());
                Poll::Pending
            }
        })
        .await;
    }

    /// Wait for an item to be available on the queue.
    ///
    /// If an item is available, it is returned. If no item is available after `max_wait`,
    /// an error is returned.
    pub fn receive_blocking(&self, max_wait: Duration) -> Result<T, FreeRtosError> {
        let mut item = MaybeUninit::<T>::uninit();
        let ptr = item.as_mut_ptr() as FreeRtosMutVoidPtr;

        let ret = unsafe { freertos_rs_queue_receive(self.queue, ptr, max_wait.ticks()) };

        if ret != 0 {
            Err(FreeRtosError::QueueReceiveTimeout)
        } else {
            self.send_waker.wake();
            // # Safety
            // We checked the return value of `freertos_rs_queue_receive` so the element is initialized.
            Ok(unsafe { item.assume_init() })
        }
    }

    /// Async version of `receive_blocking`.
    ///
    /// The function will .await until the queue has received the item.
    pub async fn receive(&self) -> T {
        poll_fn(|cx| {
            let mut item = MaybeUninit::<T>::uninit();
            let ptr = item.as_mut_ptr() as FreeRtosMutVoidPtr;
            let ret = unsafe { freertos_rs_queue_receive(self.queue, ptr, 0) };

            if ret == 0 {
                self.send_waker.wake();
                // # Safety
                // We checked the return value of `freertos_rs_queue_receive` so the element is initialized.
                Poll::Ready(unsafe { item.assume_init() })
            } else {
                self.receive_waker.register(cx.waker());
                Poll::Pending
            }
        })
        .await
    }
}

impl<T: Send + Sized> Drop for Queue<T> {
    fn drop(&mut self) {
        unsafe {
            freertos_rs_queue_delete(self.queue);
        }
    }
}
