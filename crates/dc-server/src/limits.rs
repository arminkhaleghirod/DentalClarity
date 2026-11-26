//! In-memory rate limits. Counters reset on restart, which is acceptable for
//! a single-practice server (see docs/SECURITY.md).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Upper bound on tracked keys per limiter. Past it, expired entries are
/// pruned; if the map is still full, new keys are refused until entries
/// expire. Keys already being tracked keep working.
const MAX_KEYS: usize = 10_000;

/// Rate-limit key for a client address. IPv6 clients usually control a whole
/// /64, so they are grouped by that prefix.
pub fn ip_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// Fixed-window counter keyed by an arbitrary string.
#[derive(Debug, Default)]
pub struct WindowLimiter {
    windows: Mutex<HashMap<String, (u32, Instant)>>,
}

impl WindowLimiter {
    /// Counts one request for `key`. Returns the seconds to wait when the
    /// limit for the current window is used up or the limiter is full.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> Result<(), u64> {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("limiter lock");
        if !windows.contains_key(key) && windows.len() >= MAX_KEYS {
            windows.retain(|_, (_, start)| now.duration_since(*start) < window);
            if windows.len() >= MAX_KEYS {
                return Err(window.as_secs().max(1));
            }
        }
        let entry = windows.entry(key.to_owned()).or_insert((0, now));
        if now.duration_since(entry.1) >= window {
            *entry = (0, now);
        }
        if entry.0 >= limit {
            let wait = window.saturating_sub(now.duration_since(entry.1));
            return Err(wait.as_secs().max(1));
        }
        entry.0 += 1;
        Ok(())
    }
}

/// Per-address backoff after repeated failed logins. After `FREE_FAILURES`
/// failures the wait doubles from 1 second up to 5 minutes. There is no
/// account lockout: with one shared account, a lockout would let any device
/// on the network lock out the whole practice.
#[derive(Debug, Default)]
pub struct LoginBackoff {
    by_key: Mutex<HashMap<String, Backoff>>,
}

#[derive(Debug)]
struct Backoff {
    failures: u32,
    last_failure: Instant,
    until: Option<Instant>,
}

const FREE_FAILURES: u32 = 5;
const MAX_DELAY: Duration = Duration::from_secs(300);
/// Failures older than this are forgotten.
const FAILURE_MEMORY: Duration = Duration::from_secs(15 * 60);

impl LoginBackoff {
    /// Seconds the client must still wait before another attempt, if any.
    pub fn wait(&self, ip: IpAddr) -> Option<u64> {
        let by_key = self.by_key.lock().expect("backoff lock");
        let until = by_key.get(&ip_key(ip))?.until?;
        let now = Instant::now();
        (until > now).then(|| (until - now).as_secs().max(1))
    }

    pub fn record_failure(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut by_key = self.by_key.lock().expect("backoff lock");
        if by_key.len() >= MAX_KEYS {
            by_key.retain(|_, b| {
                now.duration_since(b.last_failure) < FAILURE_MEMORY
                    || b.until.is_some_and(|u| u > now)
            });
        }
        let entry = by_key.entry(ip_key(ip)).or_insert(Backoff {
            failures: 0,
            last_failure: now,
            until: None,
        });
        if now.duration_since(entry.last_failure) >= FAILURE_MEMORY {
            entry.failures = 0;
        }
        entry.failures += 1;
        entry.last_failure = now;
        if entry.failures >= FREE_FAILURES {
            let exponent = (entry.failures - FREE_FAILURES).min(16);
            let delay = Duration::from_secs(1u64 << exponent).min(MAX_DELAY);
            entry.until = Some(now + delay);
        }
    }

    pub fn record_success(&self, ip: IpAddr) {
        self.by_key
            .lock()
            .expect("backoff lock")
            .remove(&ip_key(ip));
    }
}

/// All limiters used by the server.
#[derive(Debug, Default)]
pub struct Limits {
    pub login: WindowLimiter,
    pub login_backoff: LoginBackoff,
    /// Failed sign-ins across all clients, for the audit alert.
    pub login_failures: WindowLimiter,
    pub api: WindowLimiter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_limits_then_reports_wait() {
        let limiter = WindowLimiter::default();
        for _ in 0..3 {
            assert!(limiter.check("k", 3, Duration::from_secs(60)).is_ok());
        }
        let wait = limiter.check("k", 3, Duration::from_secs(60)).unwrap_err();
        assert!((1..=60).contains(&wait));
        assert!(limiter.check("other", 3, Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn full_limiter_refuses_new_keys_but_serves_known_ones() {
        let limiter = WindowLimiter::default();
        let window = Duration::from_secs(60);
        for i in 0..MAX_KEYS {
            limiter.check(&i.to_string(), 5, window).unwrap();
        }
        assert!(limiter.check("new", 5, window).is_err());
        assert!(limiter.check("0", 5, window).is_ok());
    }

    #[test]
    fn ipv6_is_grouped_by_slash_64() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(ip_key(a), ip_key(b));
        assert_ne!(ip_key(a), ip_key(c));
    }

    #[test]
    fn backoff_starts_after_five_failures() {
        let backoff = LoginBackoff::default();
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        for _ in 0..4 {
            backoff.record_failure(ip);
        }
        assert_eq!(backoff.wait(ip), None);
        backoff.record_failure(ip);
        assert!(backoff.wait(ip).is_some());
        backoff.record_success(ip);
        assert_eq!(backoff.wait(ip), None);
    }

    #[test]
    fn backoff_is_capped() {
        let backoff = LoginBackoff::default();
        let ip: IpAddr = "10.0.0.6".parse().unwrap();
        for _ in 0..40 {
            backoff.record_failure(ip);
        }
        assert!(backoff.wait(ip).unwrap() <= 300);
    }
}
