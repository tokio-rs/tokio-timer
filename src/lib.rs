//! Timer facilities for Tokio
//!
//! The default timer implementation is a hashed timing wheel. This structure
//! provides the best runtime characteristics for the majority of network
//! application patterns **as long as it is correctly configured**. A hashed
//! timing wheel's worst case is `O(n)` where `n` is the number of pending
//! timeouts.
//!
//! ## Example
//!
//! Here is a simple example of how to use the timer.
//!
//! ```rust
//! extern crate tokio_timer;
//! extern crate futures;
//!
//! use tokio_timer::*;
//! use futures::*;
//! use std::time::*;
//!
//! pub fn main() {
//!     // Create a new timer with default settings. While this is the easiest way
//!     // to get a timer, usually you will want to tune the config settings for
//!     // your usage patterns.
//!     let timer = Timer::default();
//!
//!     // Set a timeout that expires in 500 milliseconds
//!     let sleep = timer.sleep(Duration::from_millis(500));
//!
//!     // Use the `Future::wait` to block the current thread until `Sleep`
//!     // future completes.
//!     //
//!     sleep.wait();
//! }
//! ```
//!
//! ## Hashed Timing Wheel
//!
//! The hashed timing wheel timer is a coarse grained timer that is optimized
//! for cases where the timeout range is relatively uniform and high precision
//! is not needed. These requirements are very common with network related
//! applications as most timeouts tend to be a constant range (for example, 30
//! seconds) and timeouts are used more as a safe guard than for high
//! precision.
//!
//! The timer is inspired by the [paper by Varghese and
//! Lauck](http://www.cs.columbia.edu/~nahum/w6998/papers/ton97-timing-wheels.pdf).
//!
//! A hashed wheel timer is implemented as a vector of "slots" that represent
//! time slices. The default slot size is 100ms. As time progresses, the timer
//! walks over each slot and looks in the slot to find all timers that are due
//! to expire. When the timer reaches the end of the vector, it starts back at
//! the beginning.
//!
//! Given the fact that the timer operates in ticks, a timeout can only be as
//! precise as the tick duration. If the tick size is 100ms, any timeout
//! request that falls within that 100ms slot will be triggered at the same
//! time.
//!
//! A timer is assigned to a slot by taking the expiration instant and
//! assigning it to a slot, factoring in wrapping. When there are more than one
//! timeouts assigned to a given slot, they are stored in a linked list.
//!
//! This structure allows constant time timer operations **as long as timeouts
//! don't collide**. In other words, if two timeouts are set to expire at
//! exactly `num-slots * tick-duration` time apart, they will be assigned to
//! the same bucket.
//!
//! The best way to avoid collisions is to ensure that no timeout is set that
//! is for greater than `num-slots * tick-duration` into the future.
//!
//! A timer can be configured with `Builder`.
//!
//! ## Runtime details
//!
//! When creating a timer, a thread is spawned. The timing details are managed
//! on this thread. When `Timer::set_timeout` is called, a request is sent to
//! the thread over a bounded channel.
//!
//! All storage needed to run the timer is pre-allocated, which means that the
//! timer system is able to run without any runtime allocations. The one
//! exception would be if the timer's `max_capacity` is larger than the
//! `initial_capacity`, in which case timeout storage is allocated in chunks as
//! needed. Timeout storage can grow but never shrink.

#![deny(warnings, missing_docs)]

extern crate futures;
extern crate slab;

#[macro_use]
extern crate log;

mod mpmc;
mod timer;
mod wheel;
mod worker;

pub use timer::{Sleep, Timer, Timeout, TimerError, TimeoutError};

use std::cmp;
use std::time::Duration;

/// Configures and builds a `Timer`
///
/// A `Builder` is obtained by calling `wheel()`.
pub struct Builder {
    tick_duration: Option<Duration>,
    num_slots: Option<usize>,
    initial_capacity: Option<usize>,
    max_capacity: Option<usize>,
    max_timeout: Option<Duration>,
    channel_capacity: Option<usize>,
}

/// Configure and build a `Timer` backed by a hashed wheel.
pub fn wheel() -> Builder {
    Builder {
        tick_duration: None,
        num_slots: None,
        initial_capacity: None,
        max_capacity: None,
        max_timeout: None,
        channel_capacity: None,
    }
}

impl Builder {
    fn get_tick_duration(&self) -> Duration {
        self.tick_duration.unwrap_or(Duration::from_millis(100))
    }

    /// Set the timer tick duration.
    ///
    /// See the crate docs for more detail.
    ///
    /// Defaults to 100ms.
    pub fn tick_duration(mut self, tick_duration: Duration) -> Self {
        self.tick_duration = Some(tick_duration);
        self
    }

    fn get_num_slots(&self) -> usize {
        // About 6 minutes at a 100 ms tick size
        self.num_slots.unwrap_or(4_096)
    }

    /// Set the number of slots in the timer wheel.
    ///
    /// See the crate docs for more detail.
    ///
    /// Defaults to 4,096.
    pub fn num_slots(mut self, num_slots: usize) -> Self {
        self.num_slots = Some(num_slots);
        self
    }

    fn get_initial_capacity(&self) -> usize {
        let cap = self.initial_capacity.unwrap_or(256);
        cmp::max(cap, self.get_channel_capacity())
    }

    /// Set the initial capacity of the timer
    ///
    /// The timer's timeout storage vector will be initialized to this
    /// capacity. When the capacity is reached, the storage will be doubled
    /// until `max_capacity` is reached.
    ///
    /// Default: 128
    pub fn initial_capacity(mut self, initial_capacity: usize) -> Self {
        self.initial_capacity = Some(initial_capacity);
        self
    }

    fn get_max_capacity(&self) -> usize {
        self.max_capacity.unwrap_or(4_194_304)
    }

    /// Set the max capacity of the timer
    ///
    /// The timer's timeout storage vector cannot get larger than this capacity
    /// setting.
    ///
    /// Default: 4,194,304
    pub fn max_capacity(mut self, max_capacity: usize) -> Self {
        self.max_capacity = Some(max_capacity);
        self
    }

    fn get_max_timeout(&self) -> Duration {
        let default = self.get_tick_duration() * self.get_num_slots() as u32;
        self.max_timeout.unwrap_or(default)
    }

    /// Set the max timeout duration that can be requested
    ///
    /// Setting the max timeout allows preventing the case of timeout collision
    /// in the hash wheel and helps guarantee optimial runtime characteristics.
    ///
    /// See the crate docs for more detail.
    ///
    /// Defaults to `num_slots * tick_duration`
    pub fn max_timeout(mut self, max_timeout: Duration) -> Self {
        self.max_timeout = Some(max_timeout);
        self
    }

    fn get_channel_capacity(&self) -> usize {
        self.channel_capacity.unwrap_or(128)
    }

    /// Set the timer communication channel capacity
    ///
    /// The timer channel is used to dispatch timeout requests to the timer
    /// thread. In theory, the timer thread is able to drain the channel at a
    /// very fast rate, however it is always possible for the channel to fill
    /// up.
    ///
    /// This setting indicates the max number of timeout requests that are able
    /// to be buffered before timeout requests are rejected.
    ///
    /// Defaults to 128
    pub fn channel_capacity(mut self, channel_capacity: usize) -> Self {
        self.channel_capacity = Some(channel_capacity);
        self
    }

    /// Build the configured `Timer` and return a handle to it.
    pub fn build(self) -> Timer {
        timer::build(self)
    }
}
