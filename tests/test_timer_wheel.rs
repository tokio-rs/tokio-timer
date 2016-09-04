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

    let mut t = timer.sleep(Duration::from_millis(0));
    assert_eq!(Async::Ready(()), t.poll().unwrap());
}

#[test]
fn test_delayed_timeout() {
    let timer = Timer::default();
    let dur = Duration::from_millis(200);

    for _ in 0..20 {
        let elapsed = support::time(|| {
            timer.sleep(dur)
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

    let to1 = timer.sleep(dur1);
    let to2 = timer.sleep(dur2);

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
    let timer = timer::wheel()
        .num_slots(8)
        .max_timeout(Duration::from_millis(10_000))
        .build();

    let dur1 = Duration::from_millis(200);
    let dur2 = Duration::from_millis(1000);

    let to1 = timer.sleep(dur1);
    let to2 = timer.sleep(dur2);

    let e1 = support::time(|| to1.wait().unwrap());
    let e2 = support::time(|| to2.wait().unwrap());

    e1.assert_is_about(dur1);
    e2.assert_is_about(Duration::from_millis(800));
}

#[test]
fn test_request_timeout_greater_than_max() {
    let timer = timer::wheel()
        .max_timeout(Duration::from_millis(500))
        .build();

    let to = timer.sleep(Duration::from_millis(600));
    assert!(to.wait().is_err());

    let to = timer.sleep(Duration::from_millis(500));
    assert!(to.wait().is_ok());
}
