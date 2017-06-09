use {interval, Interval, Builder, wheel};
use worker::Worker;
use wheel::{Token, Wheel};

use futures::{Future, Stream, Async, Poll};
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
#[must_use = "futures do nothing unless polled"]
#[derive(Debug)]
pub struct Sleep {
    timer: Timer,
    when: Instant,
    handle: Option<(Task, Token)>,
}

/// Allows a given `Future` to execute for a max duration
#[must_use = "futures do nothing unless polled"]
#[derive(Debug)]
pub struct Timeout<T> {
    future: Option<T>,
    sleep: Sleep,
}

/// Allows a given `Stream` to take a max duration to yield the next value.
#[derive(Debug)]
pub struct TimeoutStream<T> {
    stream: Option<T>,
    duration: Duration,
    sleep: Sleep,
}

/// The error type for timer operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerError {
    /// The requested timeout exceeds the timer's `max_timeout` setting.
    TooLong,
    /// The timer has reached capacity and cannot support new timeouts.
    NoCapacity,
}

/// The error type for timeout operations.
#[derive(Clone)]
pub enum TimeoutError<T> {
    /// An error caused by the timer
    Timer(T, TimerError),
    /// The operation timed out
    TimedOut(T),
}

pub fn build(builder: Builder) -> Timer {
    let wheel = Wheel::new(&builder);
    let worker = Worker::spawn(wheel, builder);

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
        Sleep::new(self.clone(), duration)
    }

    /// Allow the given future to execute for at most `duration` time.
    ///
    /// If the given future completes within the given time, then the `Timeout`
    /// future will complete with that result. If `duration` expires, the
    /// `Timeout` future completes with a `TimeoutError`.
    pub fn timeout<F, E>(&self, future: F, duration: Duration) -> Timeout<F>
        where F: Future<Error = E>,
              E: From<TimeoutError<F>>,
    {
        Timeout {
            future: Some(future),
            sleep: self.sleep(duration),
        }
    }

    /// Allow the given stream to execute for at most `duration` time per
    /// yielded value.
    ///
    /// If the given stream yields a value within the allocated duration, then
    /// value is returned and the timeout is reset for the next value. If the
    /// `duration` expires, then the stream will error with a `TimeoutError`.
    pub fn timeout_stream<T, E>(&self, stream: T, duration: Duration) -> TimeoutStream<T>
        where T: Stream<Error = E>,
              E: From<TimeoutError<T>>,
    {
        TimeoutStream {
            stream: Some(stream),
            duration: duration,
            sleep: self.sleep(duration),
        }
    }

    /// Creates a new interval which will fire at `dur` time into the future,
    /// and will repeat every `dur` interval after
    pub fn interval(&self, dur: Duration) -> Interval {
        interval::new(self.sleep(dur), dur)
    }

    /// Creates a new interval which will fire at the time specified by `at`,
    /// and then will repeat every `dur` interval after
    pub fn interval_at(&self, at: Instant, dur: Duration) -> Interval {
        let now = Instant::now();

        let sleep = if at > now {
            self.sleep(at - now)
        } else {
            self.sleep(Duration::from_millis(0))
        };

        interval::new(sleep, dur)
    }
}

impl Default for Timer {
    fn default() -> Timer {
        wheel().build()
    }
}

impl fmt::Debug for Timer {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Timer")
    }
}

/*
 *
 * ===== Sleep =====
 *
 */

impl Sleep {
    /// Create a new `Sleep`
    fn new(timer: Timer, duration: Duration) -> Sleep {
        Sleep {
            timer: timer,
            when: Instant::now() + duration,
            handle: None,
        }
    }

    /// Returns true if the `Sleep` is expired.
    ///
    /// A `Sleep` is expired when the requested duration has elapsed. In
    /// practice, the `Sleep` can expire slightly before the requested duration
    /// as the timer is not precise.
    ///
    /// See the crate docs for more detail.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.when - *self.timer.worker.tolerance()
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

    /// Returns a ref to the timer backing this `Sleep`
    pub fn timer(&self) -> &Timer {
        &self.timer
    }
}

impl Future for Sleep {
    type Item = ();
    type Error = TimerError;

    fn poll(&mut self) -> Poll<(), TimerError> {
        if self.is_expired() {
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
                if (self.when - Instant::now()) > *self.timer.worker.max_timeout() {
                    return Err(TimerError::TooLong);
                }

                // Get the current task handle
                let task = task::current();

                match self.timer.worker.set_timeout(self.when, task.clone()) {
                    Ok(token) => {
                        (task, token)
                    }
                    Err(task) => {
                        // The timer is overloaded, yield the current task
                        task.notify();
                        return Ok(Async::NotReady);
                    }
                }
            }
            Some((ref task, token)) => {
                if task.will_notify_current() {
                    // Nothing more to do, the notify on timeout has already
                    // been registered
                    return Ok(Async::NotReady);
                }

                let task = task::current();

                // The timeout has been moved to another task, in this case the
                // timer has to be notified
                match self.timer.worker.move_timeout(token, self.when, task.clone()) {
                    Ok(_) => (task, token),
                    Err(task) => {
                        // Overloaded timer, yield hte current task
                        task.notify();
                        return Ok(Async::NotReady);
                    }
                }
            }
        };

        // Moved out here to make the borrow checker happy
        self.handle = Some(handle);

        Ok(Async::NotReady)
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some((_, token)) = self.handle {
            self.timer.worker.cancel_timeout(token, self.when);
        }
    }
}

/*
 *
 * ===== Timeout ====
 *
 */

impl<T> Timeout<T> {
    /// Gets a reference to the underlying future in this timeout.
    ///
    /// # Panics
    ///
    /// This function panics if the underlying future has already been consumed.
    pub fn get_ref(&self) -> &T {
        self.future.as_ref().expect("the future has already been consumed")
    }

    /// Gets a mutable reference to the underlying future in this timeout.
    ///
    /// # Panics
    ///
    /// This function panics if the underlying future has already been consumed.
    pub fn get_mut(&mut self) -> &mut T {
        self.future.as_mut().expect("the future has already been consumed")
    }

    /// Consumes this timeout, returning the underlying future.
    ///
    /// # Panics
    ///
    /// This function panics if the underlying future has already been consumed.
    pub fn into_inner(self) -> T {
        self.future.expect("the future has already been consumed")
    }
}

impl<F, E> Future for Timeout<F>
    where F: Future<Error = E>,
          E: From<TimeoutError<F>>,
{
    type Item = F::Item;
    type Error = E;

    fn poll(&mut self) -> Poll<F::Item, E> {
        // First, try polling the future
        match self.future {
            Some(ref mut f) => {
                match f.poll() {
                    Ok(Async::NotReady) => {}
                    v => return v,
                }
            }
            None => panic!("cannot call poll once value is consumed"),
        }

        // Now check the timer
        match self.sleep.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(_)) => {
                // Timeout has elapsed, error the future
                let f = self.future.take().unwrap();
                Err(TimeoutError::TimedOut(f).into())
            }
            Err(e) => {
                // Something went wrong with the underlying timeout
                let f = self.future.take().unwrap();
                Err(TimeoutError::Timer(f, e).into())
            }
        }
    }
}

/*
 *
 * ===== TimeoutStream ====
 *
 */

impl<T> TimeoutStream<T> {
    /// Gets a reference to the underlying stream in this timeout.
    ///
    /// # Panics
    ///
    /// This function panics if the underlying stream has already been consumed.
    pub fn get_ref(&self) -> &T {
        self.stream.as_ref().expect("the stream has already been consumed")
    }

    /// Gets a mutable reference to the underlying stream in this timeout.
    ///
    /// # Panics
    ///
    /// This function panics if the underlying stream has already been consumed.
    pub fn get_mut(&mut self) -> &mut T {
        self.stream.as_mut().expect("the stream has already been consumed")
    }

    /// Consumes this timeout, returning the underlying stream.
    ///
    /// # Panics
    ///
    /// This function panics if the underlying stream has already been consumed.
    pub fn into_inner(self) -> T {
        self.stream.expect("the stream has already been consumed")
    }
}

impl<T, E> Stream for TimeoutStream<T>
    where T: Stream<Error = E>,
          E: From<TimeoutError<T>>,
{
    type Item = T::Item;
    type Error = E;

    fn poll(&mut self) -> Poll<Option<T::Item>, E> {
        // First, try polling the future
        match self.stream {
            Some(ref mut s) => {
                match s.poll() {
                    Ok(Async::NotReady) => {}
                    Ok(Async::Ready(Some(v))) => {
                        // Reset the timeout
                        self.sleep = Sleep::new(self.sleep.timer.clone(), self.duration);

                        // Return the value
                        return Ok(Async::Ready(Some(v)));
                    }
                    v => return v,
                }
            }
            None => panic!("cannot call poll once value is consumed"),
        }

        // Now check the timer
        match self.sleep.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(_)) => {
                // Timeout has elapsed, error the future
                let s = self.stream.take().unwrap();
                Err(TimeoutError::TimedOut(s).into())
            }
            Err(e) => {
                // Something went wrong with the underlying timeout
                let s = self.stream.take().unwrap();
                Err(TimeoutError::Timer(s, e).into())
            }
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

impl<T> fmt::Display for TimeoutError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", Error::description(self))
    }
}

impl<T> fmt::Debug for TimeoutError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", Error::description(self))
    }
}

impl<T> Error for TimeoutError<T> {
    fn description(&self) -> &str {
        use self::TimerError::*;
        use self::TimeoutError::*;

        match *self {
            Timer(_, TooLong) => "requested timeout too long",
            Timer(_, NoCapacity) => "timer out of capacity",
            TimedOut(_) => "the future timed out",
        }
    }
}

impl<T> From<TimeoutError<T>> for io::Error {
    fn from(src: TimeoutError<T>) -> io::Error {
        use self::TimerError::*;
        use self::TimeoutError::*;

        match src {
            Timer(_, TooLong) => io::Error::new(io::ErrorKind::InvalidInput, "requested timeout too long"),
            Timer(_, NoCapacity) => io::Error::new(io::ErrorKind::Other, "timer out of capacity"),
            TimedOut(_) => io::Error::new(io::ErrorKind::TimedOut, "the future timed out"),
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

impl<T> From<TimeoutError<T>> for () {
    fn from(_: TimeoutError<T>) -> () {
    }
}
