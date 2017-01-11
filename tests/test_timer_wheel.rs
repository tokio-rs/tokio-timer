extern crate futures;
extern crate tokio_timer as timer;
extern crate env_logger;

mod support;

// use futures::*;
use futures::{Future, Stream, Sink, Async};
use futures::sync::{oneshot, mpsc};
use timer::*;
use std::io;
use std::time::*;
use std::thread;

#[test]
fn test_immediate_sleep() {
    let timer = Timer::default();

    let mut t = timer.sleep(Duration::from_millis(0));
    assert_eq!(Async::Ready(()), t.poll().unwrap());
}

#[test]
fn test_delayed_sleep() {
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
fn test_setting_later_sleep_then_earlier_one() {
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
fn test_request_sleep_greater_than_max() {
    let timer = timer::wheel()
        .max_timeout(Duration::from_millis(500))
        .build();

    let to = timer.sleep(Duration::from_millis(600));
    assert!(to.wait().is_err());

    let to = timer.sleep(Duration::from_millis(500));
    assert!(to.wait().is_ok());
}

#[test]
fn test_timeout_with_future_completes_first() {
    let timer = Timer::default();
    let dur = Duration::from_millis(300);

    let (tx, rx) = oneshot::channel();
    let rx = rx.then(|res| {
        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            _ => panic!("invalid"),
        }
    });

    let to = timer.timeout(rx, dur);

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        tx.complete(Ok::<&'static str, io::Error>("done"));
    });

    assert_eq!("done", to.wait().unwrap());
}

#[test]
fn test_timeout_with_timeout_completes_first() {
    let timer = Timer::default();
    let dur = Duration::from_millis(300);

    let (tx, rx) = oneshot::channel();
    let rx = rx.then(|res| {
        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            _ => panic!("invalid"),
        }
    });

    let to = timer.timeout(rx, dur);

    let err: io::Error = to.wait().unwrap_err();
    assert_eq!(io::ErrorKind::TimedOut, err.kind());

    // Mostly to make the type inferencer happy
    tx.complete(Ok::<&'static str, io::Error>("done"));
}

#[test]
fn test_timeout_with_future_errors_first() {
    let timer = Timer::default();
    let dur = Duration::from_millis(300);

    let (tx, rx) = oneshot::channel();
    let rx = rx.then(|res| {
        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            _ => panic!("invalid"),
        }
    });

    let to = timer.timeout(rx, dur);
    let err = io::Error::new(io::ErrorKind::NotFound, "not found");

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        tx.complete(Err::<&'static str, io::Error>(err));
    });

    let err = to.wait().unwrap_err();

    assert_eq!(io::ErrorKind::NotFound, err.kind());
}

#[test]
fn test_timeout_stream_with_stream_completes_first() {
    let timer = Timer::default();
    let dur = Duration::from_millis(300);

    let (tx, rx) = mpsc::unbounded();
    let rx = rx.then(|res| {
        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            _ => panic!("invalid"),
        }
    });

    let to = timer.timeout_stream(rx, dur);

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        let tx = tx.send(Ok::<&'static str, io::Error>("one")).wait().unwrap();

        thread::sleep(Duration::from_millis(100));
        tx.send(Ok::<&'static str, io::Error>("two")).wait().unwrap();
    });

    let mut s = to.wait();

    assert_eq!("one", s.next().unwrap().unwrap());
    assert_eq!("two", s.next().unwrap().unwrap());
    assert!(s.next().is_none());
}

#[test]
fn test_timeout_stream_with_timeout_completes_first() {
    let timer = Timer::default();
    let dur = Duration::from_millis(300);

    let (tx, rx) = mpsc::unbounded();
    let rx = rx.then(|res| {
        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            _ => panic!("invalid"),
        }
    });

    let to = timer.timeout_stream(rx, dur);

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        let tx = tx.send(Ok::<&'static str, io::Error>("one")).wait().unwrap();

        thread::sleep(Duration::from_millis(100));
        let tx = tx.send(Ok::<&'static str, io::Error>("two")).wait().unwrap();

        thread::sleep(Duration::from_millis(600));

        drop(tx);
    });

    let mut s = to.wait();

    assert_eq!("one", s.next().unwrap().unwrap());
    assert_eq!("two", s.next().unwrap().unwrap());

    let err = s.next().unwrap().unwrap_err();
    assert_eq!(io::ErrorKind::TimedOut, err.kind());
}
