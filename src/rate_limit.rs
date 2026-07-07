use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

const MAX_ATTEMPTS: usize = 10;
const WINDOW_SECS: u64 = 300;

pub struct RateLimiter {
    attempts: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, ip: IpAddr) -> bool {
        let mut map = self.attempts.lock().unwrap();
        let now = Instant::now();
        let cutoff = now - std::time::Duration::from_secs(WINDOW_SECS);

        let entries = map.entry(ip).or_default();
        entries.retain(|t| *t > cutoff);

        entries.len() < MAX_ATTEMPTS
    }

    pub fn record_failure(&self, ip: IpAddr) {
        let mut map = self.attempts.lock().unwrap();
        map.entry(ip).or_default().push(Instant::now());
    }

    pub fn reset(&self, ip: IpAddr) {
        let mut map = self.attempts.lock().unwrap();
        map.remove(&ip);
    }

    pub fn cleanup(&self) {
        let mut map = self.attempts.lock().unwrap();
        let cutoff = Instant::now() - std::time::Duration::from_secs(WINDOW_SECS);
        map.retain(|_, entries| {
            entries.retain(|t| *t > cutoff);
            !entries.is_empty()
        });
    }
}
