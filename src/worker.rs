//! This code is needed in order to support a channel that can receive with a
//! timeout.

use Builder;
use mpmc::Queue;
use wheel::{Token, Wheel};
use futures::task::Task;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::thread::{self, Thread};

#[derive(Clone)]
pub struct Worker {
    tx: Arc<Tx>,
}

/// Communicate with the timer thread
struct Tx {
    chan: Arc<Chan>,
    worker: Thread,
    tolerance: Duration,
    max_timeout: Duration,
}

struct Chan {
    run: AtomicBool,
    set_timeouts: SetQueue,
    mod_timeouts: ModQueue,
}

/// Messages sent on the `set_timeouts` exchange
struct SetTimeout(Instant, Task);

/// Messages sent on the `mod_timeouts` queue
enum ModTimeout {
    Move(Token, Instant, Task),
    Cancel(Token, Instant),
}

type SetQueue = Queue<SetTimeout, Token>;
type ModQueue = Queue<ModTimeout, ()>;

impl Worker {
    /// Spawn a worker, returning a handle to allow communication
    pub fn spawn(mut wheel: Wheel, builder: &Builder) -> Worker {
        let tolerance = builder.get_tick_duration();
        let max_timeout = builder.get_max_timeout();
        let capacity = builder.get_channel_capacity();

        // Assert that the wheel has at least capacity available timeouts
        assert!(wheel.available() >= capacity);

        let chan = Arc::new(Chan {
            run: AtomicBool::new(true),
            set_timeouts: Queue::with_capacity(capacity, || wheel.reserve().unwrap()),
            mod_timeouts: Queue::with_capacity(capacity, || ()),
        });

        let chan2 = chan.clone();

        // Spawn the worker thread
        let t = thread::spawn(move || run(chan2, wheel));

        Worker {
            tx: Arc::new(Tx {
                chan: chan,
                worker: t.thread().clone(),
                tolerance: tolerance,
                max_timeout: max_timeout,
            }),
        }
    }

    /// The earliest a timeout can fire before the requested `Instance`
    pub fn tolerance(&self) -> &Duration {
        &self.tx.tolerance
    }

    pub fn max_timeout(&self) -> &Duration {
        &self.tx.max_timeout
    }

    /// Set a timeout
    pub fn set_timeout(&self, when: Instant, task: Task) -> Result<Token, Task> {
        self.tx.chan.set_timeouts.push(SetTimeout(when, task))
            .and_then(|ret| {
                // Unpark the timer thread
                self.tx.worker.unpark();
                Ok(ret)
            })
            .map_err(|SetTimeout(_, task)| task)
    }

    /// Move a timeout
    pub fn move_timeout(&self, token: Token, when: Instant, task: Task) -> Result<(), Task> {
        self.tx.chan.mod_timeouts.push(ModTimeout::Move(token, when, task))
            .and_then(|ret| {
                self.tx.worker.unpark();
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
        let _ = self.tx.chan.mod_timeouts.push(ModTimeout::Cancel(token, instant));
    }
}

fn run(chan: Arc<Chan>, mut wheel: Wheel) {
    while chan.run.load(Ordering::Relaxed) {
        let now = Instant::now();

        // Fire off all expired timeouts
        while let Some(task) = wheel.poll(now) {
            task.unpark();
        }

        // As long as the wheel has capacity to manage new timeouts, read off
        // of the queue.
        while let Some(token) = wheel.reserve() {
            match chan.set_timeouts.pop(token) {
                Ok((SetTimeout(when, task), token)) => {
                    wheel.set_timeout(token, when, task);
                }
                Err(token) => {
                    wheel.release(token);
                    break;
                }
            }
        }

        loop {
            match chan.mod_timeouts.pop(()) {
                Ok((ModTimeout::Move(token, when, task), _)) => {
                    wheel.move_timeout(token, when, task);
                }
                Ok((ModTimeout::Cancel(token, when), _)) => {
                    wheel.cancel(token, when);
                }
                Err(_) => break,
            }
        }

        // Update `now` in case the tick was extra long for some reason
        let now = Instant::now();

        if let Some(next) = wheel.next_timeout() {
            if next > now {
                thread::park_timeout(next - now);
            }
        } else {
            thread::park();
        }
    }
}

impl Drop for Tx {
    fn drop(&mut self) {
        self.chan.run.store(false, Ordering::Relaxed);
        self.worker.unpark();
    }
}
