//! Rust wrappers around freeRTOS queues. Provide async compatibility.
use core::future::poll_fn;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::task::Poll;

use atomic_waker::AtomicWaker;

use crate::base::{
    freertos_rs_queue_create, freertos_rs_queue_delete, freertos_rs_queue_messages_waiting,
    freertos_rs_queue_receive, freertos_rs_queue_send, freertos_rs_queue_send_isr,
    FreeRtosMutVoidPtr, FreeRtosVoidPtr, FreeRtosQueueHandle,
};
use crate::isr::InterruptContext;
use crate::units::Duration;
use crate::FreeRtosError;

/// A queue with a finite size. The items are owned by the queue and are
/// copied.
#[derive(Debug)]
pub struct Queue<T: Send + Sized> {
    handle: FreeRtosQueueHandle,
    item_type: PhantomData<T>,
}

unsafe impl<T: Send + Sized> Send for Queue<T> {}
unsafe impl<T: Send + Sized> Sync for Queue<T> {}

impl<T: Send + Sized> Queue<T> {
    pub fn new(max_size: usize) -> Result<Queue<T>, FreeRtosError> {
        let item_size = core::mem::size_of::<T>();

        let handle = unsafe { freertos_rs_queue_create(max_size as u32, item_size as u32) };

        if handle.is_null() {
            return Err(FreeRtosError::OutOfMemory);
        }

        Ok(Queue {
            handle,
            item_type: PhantomData,
        })
    }

    /// # Safety
    ///
    /// `handle` must be a valid FreeRTOS regular queue handle (not semaphore or mutex).
    ///
    /// The item size of the queue must match the size of `T`.
    ///
    /// The queue will be deleted on drop.
    #[inline]
    pub unsafe fn from_raw_handle(handle: FreeRtosQueueHandle) -> Self {
        Self {
            handle,
            item_type: PhantomData,
        }
    }

    #[inline]
    pub fn raw_handle(&self) -> FreeRtosQueueHandle {
        self.handle
    }

    /// Send an item to the end of the queue. Wait for the queue to have empty space for it.
    pub fn send(&self, item: T, max_wait: Duration) -> Result<(), FreeRtosError> {
        match unsafe {
            freertos_rs_queue_send(
                self.handle,
                &item as *const _ as FreeRtosVoidPtr,
                max_wait.ticks(),
            )
        } {
            0 => Ok(()),
            _ => Err(FreeRtosError::QueueSendTimeout),
        }
    }

    /// Send an item to the end of the queue, from an interrupt.
    pub fn send_from_isr(
        &self,
        context: &mut InterruptContext,
        item: T,
    ) -> Result<(), FreeRtosError> {
        match unsafe {
            freertos_rs_queue_send_isr(
                self.handle,
                &item as *const _ as FreeRtosVoidPtr,
                context.get_task_field_mut(),
            )
        } {
            0 => Ok(()),
            _ => Err(FreeRtosError::QueueFull),
        }
    }

    /// Wait for an item to be available on the queue.
    pub fn receive(&self, max_wait: Duration) -> Result<T, FreeRtosError> {
        let mut buff = unsafe { core::mem::zeroed::<T>() };

        match unsafe {
            freertos_rs_queue_receive(
                self.handle,
                &mut buff as *mut _ as FreeRtosMutVoidPtr,
                max_wait.ticks(),
            )
        } {
            0 => Ok(buff),
            _ => Err(FreeRtosError::QueueReceiveTimeout),
        }
    }

    /// Get the number of messages in the queue.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u32 {
        unsafe { freertos_rs_queue_messages_waiting(self.handle) }
    }
}

// TODO: Fix doc comment
/// A queue with a finite size. The items are owned by the queue and are
/// cloned on send. ????
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
pub struct AsyncQueue<T: Send + Sized> {
    send_waker: AtomicWaker,
    receive_waker: AtomicWaker,
    queue: Queue<T>,
}

unsafe impl<T: Send + Sized> Send for AsyncQueue<T> {}
unsafe impl<T: Send + Sized> Sync for AsyncQueue<T> {}

impl<T: Send + Sized> AsyncQueue<T> {
    /// Create a new queue capable of holding `length` items of type `T`.
    pub fn new(length: usize) -> Result<AsyncQueue<T>, FreeRtosError> {

        Ok(AsyncQueue {
            send_waker: AtomicWaker::default(),
            receive_waker: AtomicWaker::default(),
            queue: Queue::new(length)?,
        })
    }

    /// Return the number of messages waiting in the queue.
    #[inline]
    pub fn messages_waiting(&self) -> u32 {
        unsafe { freertos_rs_queue_messages_waiting(self.queue.handle) }
    }

    /// Send an item to the end of the queue.
    ///
    /// Wait for the queue to have empty space for up to `max_wait`. If `max_size` is 0,
    /// the function will return immediately if the queue is full.
    #[inline]
    pub fn send_blocking(&self, item: T, max_wait: Duration) -> Result<(), FreeRtosError> {
        let result = self.queue.send(item, max_wait);

        if result.is_ok() {
            self.receive_waker.wake();
        }

        result
    }

    /// Send an item to the end of the queue, from an interrupt.
    #[inline]
    pub fn send_from_isr(&self, context: &mut InterruptContext, item: T) -> Result<(), FreeRtosError> {
        let result = self.queue.send_from_isr(context, item);

        if result.is_ok() {
            self.receive_waker.wake();
        }

        result
    }

    /// Async version of `send_blocking`.
    ///
    /// The function will .await until the queue has space for the item.
    pub async fn send(&self, item: T) {
        poll_fn(|cx| {
            let ptr = &item as *const _ as FreeRtosVoidPtr;
            let ret = unsafe { freertos_rs_queue_send(self.queue.handle, ptr, 0) };

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
        let result = self.queue.receive(max_wait);

        if result.is_ok() {
            self.send_waker.wake();
        }

        result
    }

    /// Async version of `receive_blocking`.
    ///
    /// The function will .await until the queue has received the item.
    pub async fn receive(&self) -> T {
        poll_fn(|cx| {
            let mut item = MaybeUninit::<T>::uninit();
            let ptr = item.as_mut_ptr() as FreeRtosMutVoidPtr;
            let ret = unsafe { freertos_rs_queue_receive(self.queue.handle, ptr, 0) };

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

impl<T: Send + Sized> Drop for AsyncQueue<T> {
    fn drop(&mut self) {
        unsafe {
            freertos_rs_queue_delete(self.queue.handle);
        }
    }
}
