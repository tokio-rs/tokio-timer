use std::time::{Duration, Instant};

pub struct Elapsed {
    duration: Duration,
}

const TOLERANCE_MS: u64 = 20;

pub fn time<F: FnOnce()>(f: F) -> Elapsed {
    let now = Instant::now();

    f();

    Elapsed { duration: now.elapsed() }
}

impl Elapsed {
    pub fn assert_is_about(&self, dur: Duration) {
        let tolerance = Duration::from_millis(TOLERANCE_MS);

        if self.duration > dur {
            assert!(self.duration - dur <= tolerance, "expect={:?}; actual={:?}", dur, self.duration);
        } else {
            assert!(dur - self.duration <= tolerance, "expect={:?}; actual={:?}", dur, self.duration);
        }
    }
}
