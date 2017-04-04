use futures::{Future, Stream, Async, Poll};

use {Sleep, TimerError};

use std::time::Duration;

/// A stream representing notifications at fixed interval
///
/// Intervals are created through `Timer::interval`.
#[derive(Debug)]
pub struct Interval {
    sleep: Sleep,
    duration: Duration,
}

/// Create a new interval
pub fn new(sleep: Sleep, dur: Duration) -> Interval {
    Interval {
        sleep: sleep,
        duration: dur,
    }
}

impl Stream for Interval {
    type Item = ();
    type Error = TimerError;

    fn poll(&mut self) -> Poll<Option<()>, TimerError> {
        let _ = try_ready!(self.sleep.poll());

        // Reset the timeout
        self.sleep = self.sleep.timer().sleep(self.duration);

        Ok(Async::Ready(Some(())))
    }
}
