//! In-memory rate limits. Counters reset on restart, which is acceptable for
//! a single-practice server (see docs/SECURITY.md).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Upper bound on tracked keys per limiter. Past it, expired entries are
/// pruned first; what happens next depends on [`WhenFull`].
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

/// Removes the entry whose `started` time is earliest, to make room in a
/// full map.
fn evict_oldest<V>(map: &mut HashMap<String, V>, started: impl Fn(&V) -> Instant) {
    let oldest = map
        .iter()
        .min_by_key(|(_, v)| started(v))
        .map(|(k, _)| k.clone());
    if let Some(oldest) = oldest {
        map.remove(&oldest);
    }
}

/// What a full limiter does with a key it does not track yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WhenFull {
    /// Refuse new keys until entries expire. Used for limits keyed by client
    /// address, where evicting would let one client reset another's count.
    #[default]
    Refuse,
    /// Drop the entry whose window started first. Used for limits keyed by a
    /// session or visitor scope, whose creation is already limited per
    /// address, so filling the map must not lock out new sessions.
    EvictOldest,
}

/// Fixed-window counter keyed by an arbitrary string.
#[derive(Debug, Default)]
pub struct WindowLimiter {
    windows: Mutex<HashMap<String, (u32, Instant)>>,
    when_full: WhenFull,
}

impl WindowLimiter {
    /// A limiter that evicts its oldest entry when full.
    pub fn evicting() -> Self {
        Self {
            windows: Mutex::default(),
            when_full: WhenFull::EvictOldest,
        }
    }

    /// Counts one request for `key`. Returns the seconds to wait when the
    /// limit for the current window is used up or the limiter is full.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> Result<(), u64> {
        self.take(key, 1, limit, window)
    }

    /// Counts `amount` units for `key`, such as characters. Refuses without
    /// counting anything when the window would pass `limit`.
    pub fn take(&self, key: &str, amount: u32, limit: u32, window: Duration) -> Result<(), u64> {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("limiter lock");
        if !windows.contains_key(key) && windows.len() >= MAX_KEYS {
            windows.retain(|_, (_, start)| now.duration_since(*start) < window);
            if windows.len() >= MAX_KEYS {
                match self.when_full {
                    WhenFull::Refuse => return Err(window.as_secs().max(1)),
                    WhenFull::EvictOldest => evict_oldest(&mut windows, |(_, start)| *start),
                }
            }
        }
        let entry = windows.entry(key.to_owned()).or_insert((0, now));
        if now.duration_since(entry.1) >= window {
            *entry = (0, now);
        }
        if entry.0.saturating_add(amount) > limit {
            let wait = window.saturating_sub(now.duration_since(entry.1));
            return Err(wait.as_secs().max(1));
        }
        entry.0 += amount;
        Ok(())
    }

    /// Gives back `amount` units taken for `key`, for work that did not
    /// happen after all.
    pub fn refund(&self, key: &str, amount: u32) {
        if let Some(entry) = self.windows.lock().expect("limiter lock").get_mut(key) {
            entry.0 = entry.0.saturating_sub(amount);
        }
    }

    /// Moves the count of `from` to `to`, so a rotated session token keeps
    /// the usage of the token it replaced.
    pub fn transfer(&self, from: &str, to: &str) {
        let mut windows = self.windows.lock().expect("limiter lock");
        if let Some(entry) = windows.remove(from) {
            windows.insert(to.to_owned(), entry);
        }
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

/// All limiters used by the server, one per purpose so that filling one
/// cannot block another (docs/SECURITY.md, Rate limits).
#[derive(Debug)]
pub struct Limits {
    /// Sign-in attempts per client address.
    pub login: WindowLimiter,
    pub login_backoff: LoginBackoff,
    /// Failed sign-ins across all clients, for the audit alert.
    pub login_failures: WindowLimiter,
    /// New demo visitor sessions per client address.
    pub visitor: WindowLimiter,
    /// Staff API requests per session.
    pub staff_api: WindowLimiter,
    /// Present API requests per present session.
    pub present_api: WindowLimiter,
    /// Exit requests per present session.
    pub present_exit: WindowLimiter,
    /// Exit requests per client address.
    pub present_exit_ip: WindowLimiter,
    /// Staff edits per demo visitor scope.
    pub edits: WindowLimiter,
    /// Synthesized narration characters per demo visitor scope.
    pub narration_scope: WindowLimiter,
    /// Synthesized narration characters per client address.
    pub narration_address: WindowLimiter,
    /// Questions asked per present session.
    pub qa_session: WindowLimiter,
    /// Questions asked per demo visitor scope.
    pub qa_scope: WindowLimiter,
    /// Questions asked per client address in demo mode.
    pub qa_address: WindowLimiter,
    /// Where each rotated demo scope went, and when. Work started under the
    /// old token, such as the narration pre-warm, resolves its scope here at
    /// charge time, so it keeps charging the visitor after an exit or unlock.
    scope_moves: Mutex<HashMap<String, (String, Instant)>>,
}

/// How long a scope move is remembered: the narration scope window, after
/// which the old key's count would have reset anyway.
const SCOPE_MOVE_MEMORY: Duration = Duration::from_secs(60 * 60);
/// Most moves followed from one scope, in case of a cycle.
const MAX_SCOPE_HOPS: usize = 16;

impl Default for Limits {
    fn default() -> Self {
        Self {
            login: WindowLimiter::default(),
            login_backoff: LoginBackoff::default(),
            login_failures: WindowLimiter::default(),
            visitor: WindowLimiter::default(),
            staff_api: WindowLimiter::evicting(),
            present_api: WindowLimiter::evicting(),
            present_exit: WindowLimiter::evicting(),
            present_exit_ip: WindowLimiter::default(),
            edits: WindowLimiter::evicting(),
            narration_scope: WindowLimiter::evicting(),
            narration_address: WindowLimiter::default(),
            qa_session: WindowLimiter::evicting(),
            qa_scope: WindowLimiter::evicting(),
            qa_address: WindowLimiter::default(),
            scope_moves: Mutex::default(),
        }
    }
}

impl Limits {
    /// Carries per-scope usage over when a demo visitor's token rotates on
    /// leaving Present mode, so leaving does not reset the edit, narration
    /// and question quotas.
    pub fn transfer_scope(&self, from: &str, to: &str) {
        self.edits.transfer(from, to);
        self.narration_scope.transfer(from, to);
        self.qa_scope.transfer(from, to);
        let now = Instant::now();
        let mut moves = self.scope_moves.lock().expect("scope lock");
        if moves.len() >= MAX_KEYS {
            moves.retain(|_, (_, at)| now.duration_since(*at) < SCOPE_MOVE_MEMORY);
            if moves.len() >= MAX_KEYS {
                evict_oldest(&mut moves, |(_, at)| *at);
            }
        }
        moves.insert(from.to_owned(), (to.to_owned(), now));
    }

    /// The scope that `scope` has become through token rotations in the
    /// last hour, or `scope` itself when it has not moved.
    pub fn current_scope(&self, scope: &str) -> String {
        let now = Instant::now();
        let moves = self.scope_moves.lock().expect("scope lock");
        let mut current = scope;
        for _ in 0..MAX_SCOPE_HOPS {
            match moves.get(current) {
                Some((to, at)) if now.duration_since(*at) < SCOPE_MOVE_MEMORY => current = to,
                _ => break,
            }
        }
        current.to_owned()
    }
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
    fn evicting_limiter_keeps_accepting_new_keys_when_full() {
        let limiter = WindowLimiter::evicting();
        let window = Duration::from_secs(60);
        for i in 0..MAX_KEYS {
            limiter.check(&i.to_string(), 1, window).unwrap();
        }
        assert!(limiter.check("1", 1, window).is_err());
        // A new session key still works, by dropping the oldest entry.
        assert!(limiter.check("new", 1, window).is_ok());
        assert!(limiter.check("new", 1, window).is_err());
        assert_eq!(limiter.windows.lock().unwrap().len(), MAX_KEYS);
    }

    #[test]
    fn take_counts_amounts_refunds_and_transfers() {
        let limiter = WindowLimiter::default();
        let hour = Duration::from_secs(3600);
        limiter.take("a", 300, 1000, hour).unwrap();
        limiter.take("a", 700, 1000, hour).unwrap();
        // Refused without counting.
        assert!(limiter.take("a", 1, 1000, hour).is_err());
        limiter.refund("a", 100);
        assert!(limiter.take("a", 101, 1000, hour).is_err());
        limiter.take("a", 100, 1000, hour).unwrap();
        limiter.transfer("a", "b");
        assert!(limiter.take("b", 1, 1000, hour).is_err());
        assert!(limiter.take("a", 1000, 1000, hour).is_ok());
    }

    #[test]
    fn rotated_scopes_resolve_to_the_newest_token() {
        let limits = Limits::default();
        let hour = Duration::from_secs(3600);
        limits.narration_scope.take("a", 10, 100, hour).unwrap();
        assert_eq!(limits.current_scope("a"), "a");
        limits.transfer_scope("a", "b");
        limits.transfer_scope("b", "c");
        assert_eq!(limits.current_scope("a"), "c");
        assert_eq!(limits.current_scope("b"), "c");
        assert_eq!(limits.current_scope("other"), "other");
        // The usage followed the moves.
        assert!(limits.narration_scope.take("c", 91, 100, hour).is_err());
        // A cycle stops after a bounded number of hops.
        limits.transfer_scope("c", "a");
        let _ = limits.current_scope("a");
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
