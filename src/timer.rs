use {Builder, wheel};
use worker::Worker;
use wheel::{Token, Wheel};
use futures::{Future, Async, Poll};
use futures::task::{self, Task};
use std::{fmt, io};
use std::error::Error;
use std::time::{Duration, Instant};

/// A facility for scheduling timeouts
#[derive(Clone)]
pub struct Timer {
    worker: Worker,
}

/// A `Future` that does nothing and completes after the requested duration
pub struct Sleep {
    worker: Worker,
    when: Instant,
    handle: Option<(Task, Token)>,
}

/// Allows a given `Future` to execute for a max duration
pub struct Timeout<T> {
    future: T,
    sleep: Sleep,
}

/// The error type for timer operations.
#[derive(Debug, Clone)]
pub enum TimerError {
    /// The requested timeout exceeds the timer's `max_timeout` setting.
    TooLong,
    /// The timer has reached capacity and cannot support new timeouts.
    NoCapacity,
}

/// The error type for timeout operations.
#[derive(Debug, Clone)]
pub enum TimeoutError {
    /// An error caused by the timer
    Timer(TimerError),
    /// The operation timed out
    TimedOut,
}

pub fn build(builder: Builder) -> Timer {
    let wheel = Wheel::new(&builder);
    let worker = Worker::spawn(wheel, &builder);

    Timer { worker: worker }
}

/*
 *
 * ===== Timer =====
 *
 */

impl Timer {
    /// Returns a future that completes once the given instant has been reached
    pub fn sleep(&self, duration: Duration) -> Sleep {
        Sleep {
            worker: self.worker.clone(),
            when: Instant::now() + duration,
            handle: None,
        }
    }

    /// Allow the given future to execute for at much `duration` time.
    ///
    /// If the given future completes within the given time, then the `Timeout`
    /// future will complete with that result. If `duration` expires, the
    /// `Timeout` future completes with a `TimeoutError`.
    pub fn timeout<F, E>(&self, future: F, duration: Duration) -> Timeout<F>
        where F: Future<Error = E>,
              E: From<TimeoutError>,
    {
        Timeout {
            future: future,
            sleep: self.sleep(duration),
        }
    }
}

impl Default for Timer {
    fn default() -> Timer {
        wheel().build()
    }
}

/*
 *
 * ===== Sleep =====
 *
 */

impl Sleep {
    /// Returns true if the `Sleep` is expired.
    ///
    /// A `Sleep` is expired when the requested duration has elapsed. In
    /// practice, the `Sleep` can expire slightly before the requested duration
    /// as the timer is not precise.
    ///
    /// See the crate docs for more detail.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.when - *self.worker.tolerance()
    }

    /// Returns the duration remaining
    pub fn remaining(&self) -> Duration {
        let now = Instant::now();

        if now >= self.when {
            Duration::from_millis(0)
        } else {
            self.when - now
        }
    }
}

impl Future for Sleep {
    type Item = ();
    type Error = TimerError;

    fn poll(&mut self) -> Poll<(), TimerError> {
        trace!("Sleep::poll; when={:?}", self.when);

        if self.is_expired() {
            trace!("  --> expired; returning");
            return Ok(Async::Ready(()));
        }

        // The `Sleep` has not expired, so perform any necessary operations
        // with the timer worker in order to get notified after the requested
        // instant.

        let handle = match self.handle {
            None => {
                // An wakeup request has not yet been sent to the timer. Before
                // doing so, check to ensure that the requested duration does
                // not exceed the `max_timeout` duration
                if (self.when - Instant::now()) > *self.worker.max_timeout() {
                    return Err(TimerError::TooLong);
                }

                trace!("  --> no handle; parking");

                // Get the current task handle
                let task = task::park();

                match self.worker.set_timeout(self.when, task.clone()) {
                    Ok(token) => {
                        trace!("  --> timeout set; token={:?}", token);
                        (task, token)
                    }
                    Err(task) => {
                        // The timer is overloaded, yield the current task
                        task.unpark();
                        return Ok(Async::NotReady);
                    }
                }
            }
            Some((ref task, token)) => {
                if task.is_current() {
                    trace!("  --> handle current -- NotReady; token={:?}; now={:?}", token, Instant::now());

                    // Nothing more to do, the notify on timeout has already
                    // been registered
                    return Ok(Async::NotReady);
                }

                trace!("  --> timeout moved -- notifying timer; when={:?}", self.when);

                let task = task::park();

                // The timeout has been moved to another task, in this case the
                // timer has to be notified
                match self.worker.move_timeout(token, self.when, task.clone()) {
                    Ok(_) => (task, token),
                    Err(task) => {
                        // Overloaded timer, yield hte current task
                        task.unpark();
                        return Ok(Async::NotReady);
                    }
                }
            }
        };

        trace!("  --> tracking handle");

        // Moved out here to make the borrow checker happy
        self.handle = Some(handle);

        Ok(Async::NotReady)
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some((_, token)) = self.handle {
            self.worker.cancel_timeout(token, self.when);
        }
    }
}

/*
 *
 * ===== Timeout ====
 *
 */

impl<F, E> Future for Timeout<F>
    where F: Future<Error = E>,
          E: From<TimeoutError>,
{
    type Item = F::Item;
    type Error = E;

    fn poll(&mut self) -> Poll<F::Item, E> {
        match self.future.poll() {
            Ok(Async::NotReady) => {},
            v => return v,
        }

        match self.sleep.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(_)) => Err(TimeoutError::TimedOut.into()),
            Err(e) => Err(TimeoutError::Timer(e).into()),
        }
    }
}

/*
 *
 * ===== Errors =====
 *
 */

impl fmt::Display for TimerError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", Error::description(self))
    }
}

impl Error for TimerError {
    fn description(&self) -> &str {
        match *self {
            TimerError::TooLong => "requested timeout too long",
            TimerError::NoCapacity => "timer out of capacity",
        }
    }
}

impl fmt::Display for TimeoutError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", Error::description(self))
    }
}

impl Error for TimeoutError {
    fn description(&self) -> &str {
        use self::TimerError::*;
        use self::TimeoutError::*;

        match *self {
            Timer(TooLong) => "requested timeout too long",
            Timer(NoCapacity) => "timer out of capacity",
            TimedOut => "the future timed out",
        }
    }
}

impl From<TimeoutError> for io::Error {
    fn from(src: TimeoutError) -> io::Error {
        use self::TimerError::*;
        use self::TimeoutError::*;

        match src {
            Timer(TooLong) => io::Error::new(io::ErrorKind::InvalidInput, "requested timeout too long"),
            Timer(NoCapacity) => io::Error::new(io::ErrorKind::Other, "timer out of capacity"),
            TimedOut => io::Error::new(io::ErrorKind::TimedOut, "the future timed out"),
        }
    }
}

impl From<TimerError> for io::Error {
    fn from(src: TimerError) -> io::Error {
        io::Error::new(io::ErrorKind::Other, src)
    }
}

impl From<TimerError> for () {
    fn from(_: TimerError) -> () {
    }
}

impl From<TimeoutError> for () {
    fn from(_: TimeoutError) -> () {
    }
}
