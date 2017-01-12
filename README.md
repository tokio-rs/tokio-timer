# tokio-timer

Timer facilities for Tokio

[![Build Status](https://travis-ci.org/tokio-rs/tokio-timer.svg?branch=master)](https://travis-ci.org/tokio-rs/tokio-timer)
[![Crates.io](https://img.shields.io/crates/v/tokio-timer.svg?maxAge=2592000)](https://crates.io/crates/tokio-timer)

[Documentation](https://docs.rs/tokio-timer) |
[Gitter](https://gitter.im/tokio-rs/tokio)

## Usage

First, add this to your `Cargo.toml`:

```toml
[dependencies]
tokio-timer = { git = "https://github.com/tokio-rs/tokio-timer" }
```

Next, add this to your crate:

```rust
extern crate tokio_timer;
```

## What is tokio-timer?

This crate provides timer facilities for usage with Tokio. Currently,
the only timer implementation is a hashed timing wheel, but will provide
a binary heap based timer at some point.

A timer provides the ability to set a timeout, represented as a future.
When the timeout is reached, the future completes. This can be
implemented very efficiently and avoiding runtime allocations.

### Hashed Timing Wheel

Inspired by the [paper by Varghese and
Lauck](http://www.cs.columbia.edu/~nahum/w6998/papers/ton97-timing-wheels.pdf),
the hashed timing wheel is a great choice for the usage pattern commonly
found when writing network applications.

# License

`tokio-timer` is primarily distributed under the terms of both the MIT
license and the Apache License (Version 2.0), with portions covered by
various BSD-like licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.

The MPMC queue implementation is inspired from
[1024cores](http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue),
see LICENSE-MPMC for details.
