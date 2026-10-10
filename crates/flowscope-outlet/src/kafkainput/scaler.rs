// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Automatic scaling of the Kafka workers. Port of
//! `outlet/kafkainput/scaler.go`.
//!
//! The decisions are made by [`Scaler`], a state machine with an explicit
//! clock, so they can be tested without waiting. [`run_scaler`] drives it
//! from a thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

/// A request to scale the workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleRequest {
    /// More workers would help.
    Increase,
    /// Fewer workers would do.
    Decrease,
    /// The number of workers is right.
    Steady,
}

/// Configuration of the scaler.
#[derive(Clone, Debug)]
pub struct ScalerConfiguration {
    pub min_workers: usize,
    pub max_workers: usize,
    pub increase_rate_limit: Duration,
    pub decrease_rate_limit: Duration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Initial,
    InitialIncrease,
    Steady,
}

impl State {
    /// Next worker count, using dichotomy at first.
    fn next_worker_count(&mut self, request: ScaleRequest, current: usize, min: usize, max: usize) -> usize {
        match (*self, request) {
            (State::Initial, ScaleRequest::Increase) => {
                *self = State::InitialIncrease;
                max.min((current + max).div_ceil(2))
            }
            (State::InitialIncrease, ScaleRequest::Increase) => max.min((current + max).div_ceil(2)),
            (State::InitialIncrease, ScaleRequest::Decrease) => {
                *self = State::Steady;
                min.max(current.saturating_sub(1))
            }
            (State::Steady, ScaleRequest::Increase) => max.min(current + 1),
            (State::Steady, ScaleRequest::Decrease) => min.max(current.saturating_sub(1)),
            _ => current,
        }
    }
}

/// The scaling state machine. Times are offsets from an arbitrary origin.
#[derive(Debug)]
pub struct Scaler {
    config: ScalerConfiguration,
    state: State,
    last: Option<Duration>,
    history: Vec<(ScaleRequest, Duration)>,
}

impl Scaler {
    pub fn new(config: ScalerConfiguration) -> Scaler {
        Scaler { config, state: State::default(), last: None, history: vec![] }
    }

    /// Handle a request received at `now` while `current` workers run.
    /// Returns the new number of workers when it should change. Once the
    /// scaling is done, call [`Scaler::scaled`].
    pub fn handle(&mut self, request: ScaleRequest, now: Duration, current: usize) -> Option<usize> {
        let since_last = |d: Duration| self.last.is_some_and(|l| l + d > now);
        // During the increase rate limit, ignore everything.
        if since_last(self.config.increase_rate_limit) {
            return None;
        }
        // Past it, accept increase requests.
        if request == ScaleRequest::Increase {
            let target =
                self.state.next_worker_count(request, current, self.config.min_workers, self.config.max_workers);
            self.last = Some(now);
            self.history.clear();
            return (target > current).then_some(target);
        }
        // Until the decrease rate limit, count steady requests to give them
        // a head start.
        if since_last(self.config.decrease_rate_limit) {
            if request == ScaleRequest::Steady {
                self.history.push((request, now));
            }
            return None;
        }
        // Past it, track all requests over the last decrease rate limit.
        self.history.push((request, now));
        let window_start = now.saturating_sub(self.config.decrease_rate_limit);
        let mut i = 0;
        while i < self.history.len() - 1 && self.history[i].1 < window_start {
            i += 1;
        }
        self.history.drain(..i);
        let decrease = self.history.iter().filter(|r| r.0 == ScaleRequest::Decrease).count();
        let steady = self.history.iter().filter(|r| r.0 == ScaleRequest::Steady).count();
        // Scale down when decrease requests dominate.
        if decrease > steady / 2 {
            let target = self.state.next_worker_count(
                ScaleRequest::Decrease,
                current,
                self.config.min_workers,
                self.config.max_workers,
            );
            self.last = Some(now);
            self.history.clear();
            return (target < current).then_some(target);
        }
        None
    }

    /// Record the end of a scaling operation.
    pub fn scaled(&mut self, now: Duration) {
        self.last = Some(now);
    }
}

/// Callbacks of the scaler loop.
pub struct ScalerCallbacks {
    pub worker_count: Box<dyn Fn() -> usize + Send>,
    pub increase_workers: Box<dyn Fn(usize, usize) + Send + Sync>,
    pub decrease_workers: Box<dyn Fn(usize, usize) + Send + Sync>,
}

/// Run the scaling loop in a thread until `stop` is set. Requests received
/// while scaling are discarded, so senders never wait for long.
pub fn run_scaler(
    config: ScalerConfiguration,
    callbacks: ScalerCallbacks,
    stop: Arc<AtomicBool>,
) -> SyncSender<ScaleRequest> {
    let (tx, rx) = std::sync::mpsc::sync_channel(config.max_workers.max(1));
    std::thread::Builder::new()
        .name("kafka-scaler".into())
        .spawn(move || scaler_loop(config, callbacks, stop, rx))
        .expect("spawn scaler thread");
    tx
}

fn scaler_loop(config: ScalerConfiguration, cb: ScalerCallbacks, stop: Arc<AtomicBool>, rx: Receiver<ScaleRequest>) {
    let origin = Instant::now();
    let mut scaler = Scaler::new(config);
    while !stop.load(Ordering::Relaxed) {
        let request = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let current = (cb.worker_count)();
        let Some(target) = scaler.handle(request, origin.elapsed(), current) else { continue };
        let finished = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                if target > current {
                    (cb.increase_workers)(current, target);
                } else {
                    (cb.decrease_workers)(current, target);
                }
                finished.store(true, Ordering::Release);
            });
            // Discard requests while scaling.
            while !finished.load(Ordering::Acquire) {
                let _ = rx.recv_timeout(Duration::from_millis(10));
            }
        });
        scaler.scaled(origin.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn state() {
        use ScaleRequest::{Decrease as D, Increase as I};
        let cases: &[(&[ScaleRequest], &[usize])] = &[
            (&[I], &[9]),
            (&[I, I, I, D, D, I], &[9, 13, 15, 14, 13, 14]),
            (&[I, I, D, D, D, D, D, D], &[9, 13, 12, 11, 10, 9, 8, 7]),
            (&[D, I, I, D, D, D, D, D, D], &[1, 9, 13, 12, 11, 10, 9, 8, 7]),
            (&[D], &[1]),
            (&[I, I, I, I, I, I], &[9, 13, 15, 16, 16, 16]),
        ];
        for (requests, expected) in cases {
            let mut state = State::default();
            let mut current = 1;
            let got: Vec<usize> = requests
                .iter()
                .map(|r| {
                    current = state.next_worker_count(*r, current, 1, 16);
                    current
                })
                .collect();
            assert_eq!(&got, expected, "{requests:?}");
        }
    }

    /// Port of TestScalerRateLimiter with a simulated clock.
    #[test]
    fn rate_limiter() {
        use ScaleRequest::{Decrease as D, Increase as I, Steady as S};
        let scaler = Scaler::new(ScalerConfiguration {
            min_workers: 1,
            max_workers: 15,
            increase_rate_limit: Duration::from_secs(60),
            decrease_rate_limit: Duration::from_secs(300),
        });
        struct Sim {
            scaler: Scaler,
            current: usize,
            got: Vec<usize>,
        }
        impl Sim {
            fn send(&mut self, now: Duration, r: ScaleRequest) {
                if let Some(target) = self.scaler.handle(r, now, self.current) {
                    self.current = target;
                    self.got.push(target);
                    self.scaler.scaled(now);
                }
            }
        }
        let mut sim = Sim { scaler, current: 1, got: vec![] };
        let mut now = Duration::ZERO;
        let s = Duration::from_secs;
        macro_rules! check {
            ($expected:expr) => {{
                now += Duration::from_millis(1);
                assert_eq!(sim.got, $expected);
            }};
        }

        sim.send(now, I);
        check!(vec![8]);
        for _ in 0..10 {
            now += s(5);
            sim.send(now, I);
        }
        check!(vec![8]);
        now += s(10);
        sim.send(now, I);
        check!(vec![8, 12]);
        for _ in 0..10 {
            now += s(6);
            sim.send(now, D);
        }
        check!(vec![8, 12]);
        for _ in 0..39 {
            now += s(6);
            sim.send(now, D);
        }
        check!(vec![8, 12]);
        now += s(6);
        sim.send(now, D);
        check!(vec![8, 12, 11]);
        for _ in 0..10 {
            now += s(5);
            sim.send(now, I);
        }
        check!(vec![8, 12, 11]);
        now += s(10);
        sim.send(now, I);
        check!(vec![8, 12, 11, 12]);
        for _ in 0..60 {
            now += s(1);
            sim.send(now, I);
            sim.send(now, D);
        }
        check!(vec![8, 12, 11, 12, 13]);
        now += s(55);
        for _ in 0..4 {
            sim.send(now, I);
        }
        for _ in 0..295 {
            now += s(1);
            sim.send(now, D);
        }
        check!(vec![8, 12, 11, 12, 13, 12]);
        now += s(300);
        for _ in 0..10 {
            sim.send(now, D);
        }
        check!(vec![8, 12, 11, 12, 13, 12, 11]);
        now += s(60);
        for _ in 0..240 {
            now += s(1);
            sim.send(now, S);
        }
        for _ in 0..60 {
            now += s(1);
            sim.send(now, D);
        }
        check!(vec![8, 12, 11, 12, 13, 12, 11]);
        for _ in 0..60 {
            now += s(1);
            sim.send(now, D);
        }
        check!(vec![8, 12, 11, 12, 13, 12, 11, 10]);
        for _ in 0..30 {
            now += s(1);
            sim.send(now, D);
        }
        now += s(60);
        for _ in 0..2400 {
            now += s(1);
            sim.send(now, S);
        }
        check!(vec![8, 12, 11, 12, 13, 12, 11, 10]);
        now += s(1);
        for _ in 0..300 {
            sim.send(now, D);
        }
        check!(vec![8, 12, 11, 12, 13, 12, 11, 10, 9]);
    }

    /// Port of TestScalerDoesNotBlock, with a real (shorter) clock.
    #[test]
    fn does_not_block() {
        let workers = Arc::new(Mutex::new((1usize, false)));
        let w1 = workers.clone();
        let w2 = workers.clone();
        let w3 = workers.clone();
        let slow = move |w: &Arc<Mutex<(usize, bool)>>, to: usize| {
            w.lock().unwrap().1 = true;
            std::thread::sleep(Duration::from_millis(500));
            *w.lock().unwrap() = (to, false);
        };
        let stop = Arc::new(AtomicBool::new(false));
        let tx = run_scaler(
            ScalerConfiguration {
                min_workers: 1,
                max_workers: 16,
                increase_rate_limit: Duration::from_millis(10),
                decrease_rate_limit: Duration::from_millis(10),
            },
            ScalerCallbacks {
                worker_count: Box::new(move || w1.lock().unwrap().0),
                increase_workers: Box::new(move |_, to| slow(&w2, to)),
                decrease_workers: Box::new(move |_, to| slow(&w3, to)),
            },
            stop.clone(),
        );
        tx.send(ScaleRequest::Increase).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(workers.lock().unwrap().1, "scaling should be in progress");
        let start = Instant::now();
        for _ in 0..100 {
            tx.send(ScaleRequest::Increase).unwrap();
        }
        assert!(start.elapsed() < Duration::from_millis(300), "sending blocked");
        std::thread::sleep(Duration::from_millis(600));
        let (count, scaling) = *workers.lock().unwrap();
        assert!(!scaling, "still scaling");
        assert_eq!(count, 9);
        stop.store(true, Ordering::Relaxed);
    }
}
