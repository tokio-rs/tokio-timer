#![allow(missing_docs, dead_code)]

use std::cell::UnsafeCell;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, Release, Acquire};

pub struct Queue<T> {
    pad0: [u8; 64],
    buffer: Vec<UnsafeCell<Node<T>>>,
    mask: usize,
    pad1: [u8; 64],
    enqueue_pos: AtomicUsize,
    pad2: [u8; 64],
    dequeue_pos: AtomicUsize,
    pad3: [u8; 64],
}

struct Node<T> {
    sequence: AtomicUsize,
    value: Option<T>,
}

impl<T: Send> Queue<T> {
    pub fn with_capacity(capacity: usize) -> Queue<T> {
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
        let buffer = (0..capacity).map(|i| {
            UnsafeCell::new(Node { sequence:AtomicUsize::new(i), value: None })
        }).collect::<Vec<_>>();
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

    pub fn push(&self, value: T) -> Result<(), T> {
        let mask = self.mask;
        let mut pos = self.enqueue_pos.load(Relaxed);
        loop {
            let node = &self.buffer[pos & mask];
            let seq = unsafe { (*node.get()).sequence.load(Acquire) };
            let diff: isize = seq as isize - pos as isize;

            if diff == 0 {
                let enqueue_pos = self.enqueue_pos.compare_and_swap(pos, pos+1, Relaxed);
                if enqueue_pos == pos {
                    unsafe {
                        (*node.get()).value = Some(value);
                        (*node.get()).sequence.store(pos+1, Release);
                    }
                    break
                } else {
                    pos = enqueue_pos;
                }
            } else if diff < 0 {
                return Err(value);
            } else {
                pos = self.enqueue_pos.load(Relaxed);
            }
        }
        Ok(())
    }

    pub fn pop(&self) -> Option<T> {
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
                        let value = (*node.get()).value.take();
                        (*node.get()).sequence.store(pos + mask + 1, Release);
                        return value
                    }
                } else {
                    pos = dequeue_pos;
                }
            } else if diff < 0 {
                return None
            } else {
                pos = self.dequeue_pos.load(Relaxed);
            }
        }
    }
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Sync> Sync for Queue<T> {}
unsafe impl<T: Send> Send for Node<T> {}
unsafe impl<T: Sync> Sync for Node<T> {}
