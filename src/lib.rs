//! Timer facilities for Tokio
//!
//! ## Hashed Timing Wheel
//!
//! Inspired by the [paper by Varghese and
//! Lauck](http://www.cs.columbia.edu/~nahum/w6998/papers/ton97-timing-wheels.pdf),
//! the hashed timing wheel is a great choice for the usage pattern commonly
//! found when writing network applications.
//!
//! ## Example
//!
//! Here is a simple example of how to use the timer.
//!
//! ```rust
//! use tokio_timer::*;
//! use std::time::*;
//!
//! // Create a new timer with default settings. While this is the easiest way
//! // to get a timer, usually you will want to tune the config settings for
//! // your usage patterns.
//! let timer = Timer::default();
//!
//! // Set a timeout that expires in 500 milliseconds
//! let timeout = timer.set_timeout(Instant::now() + Duration::from_millis(500));
//!
//! // Use the `Future::wait` to block the current thread until `Timeout`
//! // future completes.
//! //
//! // timeout.wait();
//! ```

extern crate futures;
extern crate slab;

#[macro_use]
extern crate log;

mod mpmc;
mod timer;
mod wheel;
mod worker;

pub use timer::{Timer, Timeout};
use std::cmp;
use std::time::Duration;

/// Configures and builds a `Timer`
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
    pub fn get_tick_duration(&self) -> Duration {
        self.tick_duration.unwrap_or(Duration::from_millis(100))
    }

    pub fn tick_duration(mut self, tick_duration: Duration) -> Self {
        self.tick_duration = Some(tick_duration);
        self
    }

    pub fn get_num_slots(&self) -> usize {
        // About 6 minutes at a 100 ms tick size
        self.num_slots.unwrap_or(4_096)
    }

    pub fn num_slots(mut self, num_slots: usize) -> Self {
        self.num_slots = Some(num_slots);
        self
    }

    /// Gets the initial capacity of the timer
    ///
    /// Default: 128
    pub fn get_initial_capacity(&self) -> usize {
        let cap = self.initial_capacity.unwrap_or(256);
        cmp::max(cap, self.get_channel_capacity())
    }

    /// Set the initial timer capacity
    pub fn initial_capacity(mut self, initial_capacity: usize) -> Self {
        self.initial_capacity = Some(initial_capacity);
        self
    }

    /// Get the max capacity of the timer
    ///
    /// Default: 4,194,304
    pub fn get_max_capacity(&self) -> usize {
        self.max_capacity.unwrap_or(4_194_304)
    }

    pub fn max_capacity(mut self, max_capacity: usize) -> Self {
        self.max_capacity = Some(max_capacity);
        self
    }

    pub fn get_max_timeout(&self) -> Duration {
        let default = self.get_tick_duration() * self.get_num_slots() as u32;
        self.max_timeout.unwrap_or(default)
    }

    pub fn max_timeout(mut self, max_timeout: Duration) -> Self {
        self.max_timeout = Some(max_timeout);
        self
    }

    /// Gets the current channel capacity value
    ///
    /// Defaults to 128
    pub fn get_channel_capacity(&self) -> usize {
        self.channel_capacity.unwrap_or(128)
    }

    pub fn channel_capacity(mut self, channel_capacity: usize) -> Self {
        self.channel_capacity = Some(channel_capacity);
        self
    }

    pub fn build(self) -> Timer {
        timer::build(self)
    }
}
