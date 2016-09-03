#![allow(missing_docs, dead_code)]

use std::cell::UnsafeCell;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, Release, Acquire};

pub struct Queue<T, U> {
    pad0: [u8; 64],
    buffer: Vec<UnsafeCell<Node<T, U>>>,
    mask: usize,
    pad1: [u8; 64],
    enqueue_pos: AtomicUsize,
    pad2: [u8; 64],
    dequeue_pos: AtomicUsize,
    pad3: [u8; 64],
}

struct Node<T, U> {
    sequence: AtomicUsize,
    value: Option<T>,
    token: U,
}

impl<T: Send, U: Send + Copy> Queue<T, U> {
    /// Create a new `Queue` with a capacity of `capacity` and with `token`
    /// initialized using the `init` fn
    pub fn with_capacity<F>(capacity: usize, mut init: F) -> Queue<T, U>
        where F: FnMut() -> U,
    {
        // Capacity must be a power of 2 in order to be able to use a mask to
        // map a sequence number to an index.
        let capacity = if capacity < 2 || (capacity & (capacity - 1)) != 0 {
            if capacity < 2 {
                2
            } else {
                // use next power of 2 as capacity
                capacity.next_power_of_two()
            }
        } else {
            capacity
        };

        // Initialize the buffer using the init fn
        let buffer = (0..capacity)
            .map(|i| {
                UnsafeCell::new(Node {
                    sequence:AtomicUsize::new(i),
                    value: None,
                    token: init(),
                })
            })
            .collect::<Vec<_>>();

        Queue {
            pad0: [0; 64],
            buffer: buffer,
            mask: capacity-1,
            pad1: [0; 64],
            enqueue_pos: AtomicUsize::new(0),
            pad2: [0; 64],
            dequeue_pos: AtomicUsize::new(0),
            pad3: [0; 64],
        }
    }

    pub fn push(&self, value: T) -> Result<U, T> {
        let mask = self.mask;
        let mut pos = self.enqueue_pos.load(Relaxed);

        loop {
            let node = &self.buffer[pos & mask];
            let seq = unsafe { (*node.get()).sequence.load(Acquire) };
            let diff: isize = seq as isize - pos as isize;

            if diff == 0 {
                let enqueue_pos = self.enqueue_pos
                    .compare_and_swap(pos, pos+1, Relaxed);

                if enqueue_pos == pos {
                    unsafe {
                        // Set the value
                        (*node.get()).value = Some(value);

                        // Update the sequence
                        (*node.get()).sequence.store(pos+1, Release);

                        // Return the token
                        return Ok((*node.get()).token);
                    }
                } else {
                    pos = enqueue_pos;
                }
            } else if diff < 0 {
                return Err(value);
            } else {
                pos = self.enqueue_pos.load(Relaxed);
            }
        }
    }

    pub fn pop(&self, next_token: U) -> Result<(T, U), U> {
        let mask = self.mask;
        let mut pos = self.dequeue_pos.load(Relaxed);

        loop {
            let node = &self.buffer[pos & mask];
            let seq = unsafe { (*node.get()).sequence.load(Acquire) };
            let diff: isize = seq as isize - (pos + 1) as isize;

            if diff == 0 {
                let dequeue_pos = self.dequeue_pos.compare_and_swap(pos, pos+1, Relaxed);

                if dequeue_pos == pos {
                    unsafe {
                        // Get the value & token
                        let value = (*node.get()).value.take();
                        let token = (*node.get()).token;

                        // Update the sequence number & update the slot with
                        // the next token
                        (*node.get()).sequence.store(pos + mask + 1, Release);
                        (*node.get()).token = next_token;

                        return Ok((value.unwrap(), token));
                    }
                } else {
                    pos = dequeue_pos;
                }
            } else if diff < 0 {
                return Err(next_token);
            } else {
                pos = self.dequeue_pos.load(Relaxed);
            }
        }
    }
}

unsafe impl<T: Send, U: Send + Copy> Send for Queue<T, U> {}
unsafe impl<T: Sync, U: Send + Copy> Sync for Queue<T, U> {}
unsafe impl<T: Send, U: Send + Copy> Send for Node<T, U> {}
unsafe impl<T: Sync, U: Send + Copy> Sync for Node<T, U> {}
