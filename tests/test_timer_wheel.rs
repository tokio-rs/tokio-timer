extern crate futures;
extern crate tokio_timer as timer;
extern crate env_logger;

mod support;

use futures::*;
use timer::*;
use std::time::*;

#[test]
fn test_immediate_timeout() {
    let timer = Timer::default();

    let mut t = timer.set_timeout(Instant::now());
    assert_eq!(Ok(Async::Ready(())), t.poll());
}

#[test]
fn test_delayed_timeout() {
    let _ = ::env_logger::init();

    let timer = Timer::default();

    let dur = Duration::from_millis(200);

    let elapsed = support::time(|| {
        timer.set_timeout(Instant::now() + dur)
            .wait()
            .unwrap();
    });

    elapsed.assert_is_about(dur);
}
