//! This code is needed in order to support a channel that can receive with a
//! timeout.

use {mpmc};
use self::exchange::Exchange;
use wheel::{Token, Wheel};
use futures::task::Task;
use std::time::{Duration, Instant};
use std::thread::{self, Thread};

#[derive(Clone)]
pub struct Worker {
    tx: Tx,
    worker: Thread,
    tolerance: Duration,
}

/// Communicate with the timer thread
#[derive(Clone)]
struct Tx {
    set_timeouts: Exchange,
    mod_timeouts: ModQueue,
}

/// Used by the timer thread
struct Rx {
    set_timeouts: Exchange,
    mod_timeouts: ModQueue,
}

/// Messages sent on the `set_timeouts` exchange
enum SetTimeout {
    Token(Token),
    Timeout(Token, Instant, Task),
}

/// Messages sent on the `mod_timeouts` queue
enum ModTimeout {
    Move(Token, Instant, Task),
    Cancel(Token, Instant),
}

type ModQueue = mpmc::Queue<ModTimeout>;

impl Worker {
    /// Spawn a worker, returning a handle to allow communication
    pub fn spawn(mut wheel: Wheel, tolerance: Duration, capacity: usize) -> Worker {
        // Assert that the wheel has at least capacity available timeouts
        assert!(wheel.available() >= capacity);

        // Create a queue for message passing with the worker thread
        let q1 = Exchange::with_capacity(capacity, || wheel.reserve().unwrap());
        let q2 = mpmc::Queue::with_capacity(capacity);

        let rx = Rx {
            set_timeouts: q1.clone(),
            mod_timeouts: q2.clone(),
        };

        // Spawn the worker thread
        let t = thread::spawn(move || run(rx, wheel));

        Worker {
            tx: Tx {
                set_timeouts: q1,
                mod_timeouts: q2,
            },
            worker: t.thread().clone(),
            tolerance: tolerance,
        }
    }

    /// The earliest a timeout can fire before the requested `Instance`
    pub fn tolerance(&self) -> &Duration {
        &self.tolerance
    }

    /// Set a timeout
    pub fn set_timeout(&self, when: Instant, task: Task) -> Result<Token, Task> {
        self.tx.set_timeouts.push_exch(when, task)
            .and_then(|ret| {
                // Unpark the timer thread
                self.worker.unpark();
                Ok(ret)
            })
    }

    /// Move a timeout
    pub fn move_timeout(&self, token: Token, when: Instant, task: Task) -> Result<(), Task> {
        self.tx.mod_timeouts.push(ModTimeout::Move(token, when, task))
            .and_then(|ret| {
                self.worker.unpark();
                Ok(ret)
            })
            .map_err(|v| {
                match v {
                    ModTimeout::Move(_, _, task) => task,
                    _ => unreachable!(),
                }
            })

    }

    /// Cancel a timeout
    pub fn cancel_timeout(&self, token: Token, instant: Instant) {
        // The result here is ignored because:
        //
        // 1) this fn is only called when the timeout is dropping, so nothing
        //    can be done with the result
        // 2) Not being able to cancel a timeout is not a huge deal and only
        //    results in a spurious wakeup.
        //
        let _ = self.tx.mod_timeouts.push(ModTimeout::Cancel(token, instant));
    }
}

fn run(rx: Rx, mut wheel: Wheel) {
    // Run forever
    // TODO: Don't run forever
    loop {
        let now = Instant::now();

        trace!("Worker tick; now={:?}", now);

        // Fire off all expired timeouts
        while let Some(task) = wheel.poll(now) {
            trace!("  --> unparking task");
            task.unpark();
        }

        // As long as the wheel has capacity to manage new timeouts, read off
        // of the queue.
        while let Some(token) = wheel.reserve() {
            match rx.set_timeouts.pop_exch(token) {
                Ok((token, when, task)) => {
                    trace!("  --> SetTimeout; token={:?}; instant={:?}", token, when);
                    wheel.set_timeout(token, when, task);
                }
                Err(token) => {
                    wheel.release(token);
                    break;
                }
            }
        }

        loop {
            match rx.mod_timeouts.pop() {
                Some(ModTimeout::Move(token, when, task)) => {
                    trace!("  --> ModTimeout::Move; token={:?}; instant={:?}", token, when);
                    wheel.move_timeout(token, when, task);
                }
                Some(ModTimeout::Cancel(token, when)) => {
                    trace!("  --> ModTimeout::Cancel; token={:?}; instant={:?}", token, when);
                    wheel.cancel(token, when);
                }
                None => break,
            }
        }

        // Update `now` in case the tick was extra long for some reason
        let now = Instant::now();

        if let Some(next) = wheel.next_timeout() {
            thread::park_timeout(next - now);
        } else {
            thread::park();
        }
    }
}

mod exchange {
    //! A lock-free, fixed-size, exchange queue backed by a ring.
    //!
    //! This is used by the timer to reserve timeout tokens and provide them to the
    //! task requesting a new timeout. When the timer is initialized, it allocates
    //! a Slab at least as big as the add-timer exchange. It then initializes the
    //! exchange w/ Slab indexes that are then reserved for an incoming timeout.
    //!
    //! When a task requests a timeout, it pushes it's task handle and the
    //! expiration and gets the timeout token.

    /* Copyright (c) 2010-2011 Dmitry Vyukov. All rights reserved.
     * Redistribution and use in source and binary forms, with or without
     * modification, are permitted provided that the following conditions are met:
     *
     *    1. Redistributions of source code must retain the above copyright notice,
     *       this list of conditions and the following disclaimer.
     *
     *    2. Redistributions in binary form must reproduce the above copyright
     *       notice, this list of conditions and the following disclaimer in the
     *       documentation and/or other materials provided with the distribution.
     *
     * THIS SOFTWARE IS PROVIDED BY DMITRY VYUKOV "AS IS" AND ANY EXPRESS OR IMPLIED
     * WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
     * MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT
     * SHALL DMITRY VYUKOV OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
     * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
     * LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
     * PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
     * LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE
     * OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF
     * ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
     *
     * The views and conclusions contained in the software and documentation are
     * those of the authors and should not be interpreted as representing official
     * policies, either expressed or implied, of Dmitry Vyukov.
     */

    // The following code is based on the bounded mpmc queue implementation that
    // used to be in the old rust stdlib and originated from:
    // http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue

    use super::SetTimeout;
    use wheel::Token;
    use futures::task::Task;
    use std::ptr;
    use std::sync::Arc;
    use std::cell::UnsafeCell;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::{Relaxed, Release, Acquire};
    use std::time::Instant;

    pub struct Exchange {
        state: Arc<State>,
    }

    struct Node {
        sequence: AtomicUsize,
        value: SetTimeout,
    }

    #[allow(dead_code)]
    struct State {
        pad0: [u8; 64],
        buffer: Vec<UnsafeCell<Node>>,
        mask: usize,
        pad1: [u8; 64],
        enqueue_pos: AtomicUsize,
        pad2: [u8; 64],
        dequeue_pos: AtomicUsize,
        pad3: [u8; 64],
    }

    unsafe impl Send for State {}
    unsafe impl Sync for State {}

    impl State {
        fn with_capacity<F>(capacity: usize, mut init: F) -> State
            where F: FnMut() -> Token
        {
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

            let buffer = (0..capacity)
                .map(|i| {
                    UnsafeCell::new(Node {
                        sequence: AtomicUsize::new(i),
                        value: SetTimeout::Token(init()),
                    })
                })
                .collect::<Vec<_>>();

            State{
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

        fn push_exch(&self, instant: Instant, task: Task) -> Result<Token, Task> {
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
                            // Read the value
                            let token = match ptr::read(&(*node.get()).value as *const SetTimeout) {
                                SetTimeout::Token(token) => token,
                                _ => {
                                    // In case we do hit this point, leave the
                                    // memory in a consistent state
                                    ptr::write(
                                        &mut (*node.get()).value as *mut SetTimeout,
                                        SetTimeout::Token(Token(0)));

                                    unreachable!();
                                }
                            };

                            // Set the timeout info
                            ptr::write(
                                &mut (*node.get()).value as *mut SetTimeout,
                                SetTimeout::Timeout(token, instant, task));

                            // Update the sequence
                            (*node.get()).sequence.store(pos+1, Release);

                            // Return the token
                            return Ok(token);
                        }
                    } else {
                        pos = enqueue_pos;
                    }
                } else if diff < 0 {
                    return Err(task);
                } else {
                    pos = self.enqueue_pos.load(Relaxed);
                }
            }
        }

        fn pop_exch(&self, next_token: Token) -> Result<(Token, Instant, Task), Token> {
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
                            // Read the value
                            let ret = match ptr::read(&(*node.get()).value as *const SetTimeout) {
                                SetTimeout::Timeout(token, instant, task) => (token, instant, task),
                                _ => {
                                    // The memory is in a constent state...
                                    // kind of
                                    unreachable!();
                                }
                            };

                            // Write the next reserved token
                            ptr::write(
                                &mut (*node.get()).value as *mut SetTimeout,
                                SetTimeout::Token(next_token));

                            // Update the sequence
                            (*node.get()).sequence.store(pos + mask + 1, Release);

                            // Return the result
                            return Ok(ret);
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

    impl Exchange {
        pub fn with_capacity<F>(capacity: usize, init: F) -> Exchange
            where F: FnMut() -> Token,
        {
            Exchange {
                state: Arc::new(State::with_capacity(capacity, init))
            }
        }

        pub fn push_exch(&self, instant: Instant, task: Task) -> Result<Token, Task> {
            self.state.push_exch(instant, task)
        }

        pub fn pop_exch(&self, next_token: Token) -> Result<(Token, Instant, Task), Token> {
            self.state.pop_exch(next_token)
        }
    }

    impl Clone for Exchange {
        fn clone(&self) -> Exchange {
            Exchange { state: self.state.clone() }
        }
    }
}
