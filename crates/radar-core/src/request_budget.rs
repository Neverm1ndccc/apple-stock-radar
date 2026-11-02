//! Application protection budget, not a published Apple quota.
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub const CAPACITY: u32 = 6;
pub const REFILL_SECONDS: u64 = 60;
const COST_MS: u64 = REFILL_SECONDS * 1000;
const MAX_CREDIT: u64 = CAPACITY as u64 * COST_MS;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBudget {
    pub credit_ms: u64,
    pub updated_at: DateTime<Utc>,
}

impl RequestBudget {
    pub fn full(at: DateTime<Utc>) -> Self {
        Self {
            credit_ms: MAX_CREDIT,
            updated_at: at,
        }
    }
    pub fn empty(at: DateTime<Utc>) -> Self {
        Self {
            credit_ms: 0,
            updated_at: at,
        }
    }
    pub fn validate(&self) -> bool {
        self.credit_ms <= MAX_CREDIT
            && self
                .updated_at
                .checked_add_signed(Duration::milliseconds(COST_MS as i64))
                .is_some()
    }
    pub fn projected(&self, now: DateTime<Utc>) -> Self {
        let elapsed = now
            .signed_duration_since(self.updated_at)
            .num_milliseconds()
            .clamp(0, MAX_CREDIT as i64) as u64;
        Self {
            credit_ms: self.credit_ms.saturating_add(elapsed).min(MAX_CREDIT),
            updated_at: now.max(self.updated_at),
        }
    }
    pub fn remaining(&self, now: DateTime<Utc>) -> u32 {
        (self.projected(now).credit_ms / COST_MS) as u32
    }
    pub fn ready_at(&self) -> Option<DateTime<Utc>> {
        (self.credit_ms < COST_MS)
            .then(|| self.updated_at + Duration::milliseconds((COST_MS - self.credit_ms) as i64))
    }
    pub fn take(&mut self, now: DateTime<Utc>) -> bool {
        *self = self.projected(now);
        if self.credit_ms < COST_MS {
            return false;
        }
        self.credit_ms -= COST_MS;
        true
    }
}

pub fn retry_after(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let value = value.trim();
    let deadline = if value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty() {
        value
            .parse::<i64>()
            .ok()
            .and_then(Duration::try_seconds)
            .and_then(|delay| now.checked_add_signed(delay))
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    } else {
        DateTime::parse_from_rfc2822(value)
            .ok()?
            .with_timezone(&Utc)
    };
    Some(deadline.max(now))
}
