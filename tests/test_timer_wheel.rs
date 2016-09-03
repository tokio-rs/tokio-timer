extern crate futures;
extern crate tokio_timer as timer;
extern crate env_logger;

mod support;

use futures::*;
use timer::*;
use std::time::*;
use std::thread;

#[test]
fn test_immediate_timeout() {
    let timer = Timer::default();

    let mut t = timer.set_timeout(Instant::now());
    assert_eq!(Ok(Async::Ready(())), t.poll());
}

#[test]
fn test_delayed_timeout() {
    let timer = Timer::default();
    let dur = Duration::from_millis(200);

    for _ in 0..20 {
        let elapsed = support::time(|| {
            timer.set_timeout(Instant::now() + dur)
                .wait()
                .unwrap();
        });

        elapsed.assert_is_about(dur);
    }
}

#[test]
fn test_setting_later_timeout_then_earlier_one() {
    let timer = Timer::default();

    let dur1 = Duration::from_millis(500);
    let dur2 = Duration::from_millis(200);

    let to1 = timer.set_timeout(Instant::now() + dur1);
    let to2 = timer.set_timeout(Instant::now() + dur2);

    let t1 = thread::spawn(move || {
        support::time(|| to1.wait().unwrap())
    });

    let t2 = thread::spawn(move || {
        support::time(|| to2.wait().unwrap())
    });

    t1.join().unwrap().assert_is_about(dur1);
    t2.join().unwrap().assert_is_about(dur2);
}

#[test]
fn test_timer_with_looping_wheel() {
    let _ = ::env_logger::init();

    let timer = timer::wheel()
        .num_slots(8)
        .build();

    let dur1 = Duration::from_millis(200);
    let dur2 = Duration::from_millis(1000);

    let to1 = timer.set_timeout(Instant::now() + dur1);
    let to2 = timer.set_timeout(Instant::now() + dur2);

    let e1 = support::time(|| to1.wait().unwrap());
    let e2 = support::time(|| to2.wait().unwrap());

    e1.assert_is_about(dur1);
    e2.assert_is_about(Duration::from_millis(800));
}
