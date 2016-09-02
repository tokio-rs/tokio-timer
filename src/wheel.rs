use {Builder};
use futures::task::Task;
use slab::Slab;
use std::{cmp, mem, usize};
use std::time::{Instant, Duration};

pub struct Wheel {
    // Actual timer wheel itself.
    //
    // Each slot represents a fixed duration of time, and this wheel also
    // behaves like a ring buffer. All timeouts scheduled will correspond to one
    // slot and therefore each slot has a linked list of timeouts scheduled in
    // it. Right now linked lists are done through indices into the `slab`
    // below.
    //
    // Each slot also contains the next timeout associated with it (the minimum
    // of the entire linked list).
    wheel: Vec<Slot>,

    // A slab containing all the timeout entries themselves. This is the memory
    // backing the "linked lists" in the wheel above. Each entry has a prev/next
    // pointer (indices in this array) along with the data associated with the
    // timeout and the time the timeout will fire.
    slab: Slab<Entry, Token>,

    // The instant that this timer was created, through which all other timeout
    // computations are relative to.
    start: Instant,

    // State used during `poll`. The `cur_wheel_tick` field is the current tick
    // we've poll'd to. That is, all events from `cur_wheel_tick` to the
    // actual current tick in time still need to be processed.
    //
    // The `cur_slab_idx` variable is basically just an iterator over the linked
    // list associated with a wheel slot. This will get incremented as we move
    // forward in `poll`
    cur_wheel_tick: u64,

    // The next timeout to tick
    cur_slab_idx: Token,

    // Max capacity of the slab
    max_capacity: usize,

    // The duration of each tick in ms
    tick_ms: u64,

    // Mask to convert the current tick to a wheel slot
    mask: usize,
}

#[derive(Clone)]
struct Slot {
    head: Token,
    next_timeout: Option<Instant>,
}

enum Entry {
    Reserved,
    Timeout(Timeout),
}

struct Timeout {
    task: Task,
    when: Instant,
    wheel_idx: usize,
    prev: Token,
    next: Token,
}

/// Represents a slot in the timer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token(pub usize);

const EMPTY: Token = Token(usize::MAX);

impl Wheel {
    /// Creates a new timer wheel with the given configuration settings.
    pub fn new(builder: &Builder) -> Wheel {
        let num_slots = builder.get_num_slots();
        let mask = num_slots - 1;

        // Check that the number of slots requested is, in fact, a power of two
        assert!(num_slots & mask == 0);

        Wheel {
            wheel: vec![Slot { head: EMPTY, next_timeout: None }; num_slots],
            slab: Slab::with_capacity(builder.get_initial_capacity()),
            start: Instant::now(),
            cur_wheel_tick: 0,
            cur_slab_idx: EMPTY,
            max_capacity: builder.get_max_capacity(),
            tick_ms: millis(builder.get_tick_duration()),
            mask: mask,
        }
    }

    pub fn available(&self) -> usize {
        self.slab.available()
    }

    /// Reserve a slot in the timer
    pub fn reserve(&mut self) -> Option<Token> {
        // Ensure that there is enough space to reserve a new token.
        //
        // TODO: Ensure max capacity is not exceeded
        if self.slab.vacant_entry().is_none() {
            let amt = self.slab.len();
            let amt = cmp::min(amt, self.max_capacity - amt);

            if amt == 0 {
                // Reached max capacity
                return None;
            }

            self.slab.reserve_exact(amt);
        }

        // Reserve the slot
        self.slab.insert(Entry::Reserved).ok()
    }

    pub fn release(&mut self, token: Token) {
        self.slab.remove(token);
    }

    pub fn set_timeout(&mut self, token: Token, mut at: Instant, task: Task) {
        // First up, figure out where we're gonna go in the wheel. Note that if
        // we're being scheduled on or before the current wheel tick we just
        // make sure to defer ourselves to the next tick.
        let mut tick = self.time_to_ticks(at);

        if tick <= self.cur_wheel_tick {
            debug!("moving {} to {}", tick, self.cur_wheel_tick + 1);
            tick = self.cur_wheel_tick + 1;
        }

        let wheel_idx = self.ticks_to_wheel_idx(tick);
        trace!("inserting timeout at {} for {}", wheel_idx, tick);

        let actual_tick = self.start +
                          Duration::from_millis(self.tick_ms) * (tick as u32);

        trace!("actual_tick: {:?}", actual_tick);
        trace!("at:          {:?}", at);
        at = actual_tick;

        // Insert ourselves at the head of the linked list in the wheel.
        let slot = &mut self.wheel[wheel_idx];

        let prev_head = mem::replace(&mut slot.head, token);

        {
            trace!("timer wheel slab idx: {:?}", token);

            self.slab[token] = Entry::Timeout(Timeout {
                task: task,
                when: at,
                wheel_idx: wheel_idx,
                prev: EMPTY,
                next: prev_head,
            });
        }

        if prev_head != EMPTY {
            match self.slab[prev_head] {
                Entry::Timeout(ref mut v) => v.prev = slot.head,
                _ => panic!("unexpected state"),
            }
        }

        // Update the wheel slot's next timeout field.
        if at <= slot.next_timeout.unwrap_or(at) {
            debug!("updating[{}] next timeout: {:?}", wheel_idx, at);
            debug!("                    start: {:?}", self.start);
            slot.next_timeout = Some(at);
        }
    }

    /// Queries this timer to see if any timeouts are ready to fire.
    ///
    /// This function will advance the internal wheel to the time specified by
    /// `at`, returning any timeout which has happened up to that point. This
    /// method should be called in a loop until it returns `None` to ensure that
    /// all timeouts are processed.
    ///
    /// # Panics
    ///
    /// This method will panic if `at` is before the instant that this timer
    /// wheel was created.
    pub fn poll(&mut self, at: Instant) -> Option<Task> {
        let wheel_tick = self.time_to_ticks(at);

        trace!("polling {} => {}", self.cur_wheel_tick, wheel_tick);

        // Advance forward in time to the `wheel_tick` specified.
        //
        // TODO: don't visit slots in the wheel more than once
        while self.cur_wheel_tick <= wheel_tick {
            let head = self.cur_slab_idx;
            let idx = self.ticks_to_wheel_idx(self.cur_wheel_tick);
            trace!("next head[{} => {}]: {:?}",
                   self.cur_wheel_tick, wheel_tick, head);

            // If the current slot has no entries or we're done iterating go to
            // the next tick.
            if head == EMPTY {
                if head == self.wheel[idx].head {
                    self.wheel[idx].next_timeout = None;
                }
                self.cur_wheel_tick += 1;
                let idx = self.ticks_to_wheel_idx(self.cur_wheel_tick);
                self.cur_slab_idx = self.wheel[idx].head;
                continue
            }

            // If we're starting to iterate over a slot, clear its timeout as
            // we're probably going to remove entries. As we skip over each
            // element of this slot we'll restore the `next_timeout` field if
            // necessary.
            if head == self.wheel[idx].head {
                self.wheel[idx].next_timeout = None;
            }

            // Otherwise, continue iterating over the linked list in the wheel
            // slot we're on and remove anything which has expired.
            let head_timeout = {
                let timeout = self.slab[head].timeout();
                self.cur_slab_idx = timeout.next;
                timeout.when
            };

            if self.time_to_ticks(head_timeout) <= self.time_to_ticks(at) {
                let task = match self.remove_slab(head) {
                    Some(Entry::Timeout(v)) => Some(v.task),
                    _ => None,
                };

                return task;
            } else {
                let next = self.wheel[idx].next_timeout.unwrap_or(head_timeout);
                if head_timeout <= next {
                    self.wheel[idx].next_timeout = Some(head_timeout);
                }
            }
        }

        None
    }

    /// Returns the instant in time that corresponds to the next timeout
    /// scheduled in this wheel.
    pub fn next_timeout(&self) -> Option<Instant> {
        // TODO: can this be optimized to not look at the whole array?
        let mut min = None;
        for a in self.wheel.iter().filter_map(|s| s.next_timeout.as_ref()) {
            if let Some(b) = min {
                if b < a {
                    continue
                }
            }
            min = Some(a);
        }
        if let Some(min) = min {
            debug!("next timeout {:?}", min);
            debug!("now          {:?}", Instant::now());
        } else {
            debug!("next timeout never");
        }
        min.map(|t| *t)
    }

    pub fn move_timeout(&mut self, token: Token, when: Instant, task: Task) {
        match self.slab.get_mut(token) {
            Some(&mut Entry::Timeout(ref mut e)) if e.when == when => {
                e.task = task;
            }
            _ => {}
        }
    }

    /// Cancels the specified timeout.
    ///
    /// For timeouts previously registered via `insert` they can be passed back
    /// to this method to cancel the associated timeout, retrieving the value
    /// inserted if the timeout has not already fired.
    ///
    /// This method completes in O(1) time.
    ///
    /// # Panics
    ///
    /// This method may panic if `timeout` wasn't created by this timer wheel.
    pub fn cancel(&mut self, token: Token, when: Instant) {
        match self.slab.get(token) {
            Some(&Entry::Timeout(ref e)) if e.when == when => {}
            _ => return,
        }

        self.remove_slab(token);
    }

    fn remove_slab(&mut self, slab_idx: Token) -> Option<Entry> {
        debug!("removing timer slab {:?}", slab_idx);
        let mut entry = match self.slab.remove(slab_idx) {
            Some(e) => e,
            None => return None,
        };

        if let Entry::Timeout(ref mut entry) = entry {
            // Remove the node from the linked list
            if entry.prev == EMPTY {
                self.wheel[entry.wheel_idx].head = entry.next;
            } else {
                self.slab[entry.prev].timeout_mut().next = entry.next;
            }
            if entry.next != EMPTY {
                self.slab[entry.next].timeout_mut().prev = entry.prev;
            }

            if self.cur_slab_idx == slab_idx {
                self.cur_slab_idx = entry.next;
            }
        }

        return Some(entry)
    }

    fn time_to_ticks(&self, time: Instant) -> u64 {
        let dur = time - self.start;
        let ms = dur.subsec_nanos() as u64 / 1_000_000;
        let ms = dur.as_secs()
                    .checked_mul(1_000)
                    .and_then(|m| m.checked_add(ms))
                    .expect("overflow scheduling timeout");
        ms / self.tick_ms
    }

    fn ticks_to_wheel_idx(&self, ticks: u64) -> usize {
        (ticks as usize) & self.mask
    }
}

impl Entry {
    fn timeout(&self) -> &Timeout {
        match *self {
            Entry::Timeout(ref v) => v,
            _ => panic!("unexpected state"),
        }
    }

    fn timeout_mut(&mut self) -> &mut Timeout {
        match *self {
            Entry::Timeout(ref mut v) => v,
            _ => panic!("unexpected state"),
        }
    }
}

impl From<usize> for Token {
    fn from(src: usize) -> Token {
        Token(src)
    }
}

impl From<Token> for usize {
    fn from(src: Token) -> usize {
        src.0
    }
}

pub fn millis(duration: Duration) -> u64 {
    const NANOS_PER_MILLI: u32 = 1_000_000;
    const MILLIS_PER_SEC: u64 = 1_000;

    // Convert a `Duration` to milliseconds, rounding up and saturating at
    // `u64::MAX`.
    //
    // The saturating is fine because `u64::MAX` milliseconds are still many
    // million years.
    let millis = (duration.subsec_nanos() + NANOS_PER_MILLI - 1) / NANOS_PER_MILLI;
    duration.as_secs().saturating_mul(MILLIS_PER_SEC).saturating_add(millis as u64)
}
