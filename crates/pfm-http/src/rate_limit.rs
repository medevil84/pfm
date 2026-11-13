//! In-memory sliding-window rate limiter for login attempts.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Number of tracked keys above which [`RateLimiter::check`] sweeps the whole
/// map for expired entries. Small enough to keep memory bounded, large enough
/// that the sweep is rare on a normal workload.
const PRUNE_THRESHOLD: usize = 1024;

/// Hard cap on tracked keys. Past this the limiter fails closed for *new*
/// keys instead of growing: an attacker rotating usernames from many addresses
/// cannot turn the limiter itself into a memory-exhaustion vector.
const MAX_TRACKED_KEYS: usize = 50_000;

/// How many times the per-(ip, username) budget one client address gets across
/// every username in the same window.
const PER_IP_FACTOR: u32 = 6;

#[derive(Debug, Default)]
struct Window {
    attempts: HashMap<String, Vec<Instant>>,
}

impl Window {
    fn prune(&mut self, now: Instant, window: Duration) {
        self.attempts.retain(|_, attempts| {
            attempts.retain(|t| now.duration_since(*t) < window);
            !attempts.is_empty()
        });
    }

    /// Record an attempt for `key`; `true` means the caller must be rejected.
    fn hit(&mut self, key: &str, limit: u32, now: Instant, window: Duration) -> bool {
        // Global prune. Without it the map only ever grows: every distinct key
        // leaves a permanent entry. Amortized: only once the map is big.
        if self.attempts.len() >= PRUNE_THRESHOLD {
            self.prune(now, window);
        }
        if self.attempts.len() >= MAX_TRACKED_KEYS && !self.attempts.contains_key(key) {
            return true;
        }

        let entry = self.attempts.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < window);
        if entry.len() as u32 >= limit {
            return true;
        }
        entry.push(now);
        false
    }
}

/// Thread-safe in-memory rate limiter using a sliding window.
#[derive(Debug)]
pub struct RateLimiter {
    by_key: Mutex<Window>,
    by_ip: Mutex<Window>,
    limit: u32,
    ip_limit: u32,
    window: Duration,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            by_key: Mutex::new(Window::default()),
            by_ip: Mutex::new(Window::default()),
            limit,
            ip_limit: limit.saturating_mul(PER_IP_FACTOR),
            window,
        }
    }

    /// Check whether the given key (`ip:username`) is currently rate-limited.
    /// Prunes expired entries and records the current attempt.
    /// Returns `true` if the caller should be rejected.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.by_key.lock().unwrap_or_else(|e| e.into_inner());
        map.hit(key, self.limit, now, self.window)
    }

    /// Same as [`check`](Self::check) but keyed on the client address alone,
    /// with a larger budget: it bounds how many *different* usernames one
    /// address can try, which the per-username key cannot.
    pub fn check_ip(&self, ip: &str) -> bool {
        let now = Instant::now();
        let mut map = self.by_ip.lock().unwrap_or_else(|e| e.into_inner());
        map.hit(ip, self.ip_limit, now, self.window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_key_limit_applies() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        assert!(!rl.check("a"));
        assert!(!rl.check("a"));
        assert!(rl.check("a"));
        assert!(!rl.check("b"));
    }

    #[test]
    fn per_ip_limit_spans_usernames() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        let mut rejected = false;
        for _ in 0..(PER_IP_FACTOR + 1) {
            rejected = rl.check_ip("203.0.113.5");
        }
        assert!(rejected);
        assert!(!rl.check_ip("203.0.113.6"));
    }
}
