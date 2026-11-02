use crate::{Settings, Source, apple::ScanFailure};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanStatus {
    pub running: bool,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_finished_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub next_scan_at: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub stop_reason: Option<String>,
    pub checked_targets: usize,
    pub batch_count: usize,
    pub minimum_interval_seconds: Option<u64>,
    pub allowed_locales: Vec<String>,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub access_kind: String,
    pub last_http_status: Option<u16>,
    pub budget_remaining: u32,
    pub budget_capacity: u32,
    pub budget_refill_seconds: u64,
    pub budget_ready_at: Option<DateTime<Utc>>,
    pub resume_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
pub struct Scanner {
    pub status: ScanStatus,
    pub cursor: usize,
    retry_after: Option<DateTime<Utc>>,
}

impl Scanner {
    pub fn due_at(&self, settings: &Settings, minimum: u64) -> Option<DateTime<Utc>> {
        let interval = settings.effective_scan_interval(minimum) as i64;
        let last = self
            .status
            .last_finished_at
            .into_iter()
            .chain(settings.scan_last_request_at)
            .max();
        let due = last.map(|at| at + Duration::seconds(interval));
        due.into_iter()
            .chain(self.retry_after)
            .chain(settings.scan_retry_after)
            .chain(settings.budget_ready_at())
            .max()
    }
    pub fn begin(
        &mut self,
        settings: &Settings,
        minimum: u64,
        available: bool,
        now: DateTime<Utc>,
    ) -> bool {
        self.status.minimum_interval_seconds =
            available.then_some(settings.scan_minimum_interval(minimum));
        self.status.next_scan_at = self.due_at(settings, minimum);
        if !available
            || settings.mode != Source::Apple
            || !settings.scan_running
            || settings.paused
            || self.status.running
            || self.status.stop_reason.is_some()
            || self.status.next_scan_at.is_some_and(|at| at > now)
        {
            return false;
        }
        self.status.running = true;
        self.status.consecutive_failures = settings.scan_failure_count;
        self.status.last_started_at = Some(now);
        self.status.next_scan_at = None;
        true
    }
    pub fn finish(
        &mut self,
        settings: &Settings,
        minimum: u64,
        now: DateTime<Utc>,
        failure: Option<ScanFailure>,
        checked: usize,
    ) {
        self.status.running = false;
        self.status.last_finished_at = Some(now);
        self.status.checked_targets = checked;
        if let Some(failure) = failure {
            self.status.consecutive_failures = self.status.consecutive_failures.saturating_add(1);
            if failure.stops_source() {
                self.status.invalidated_at = Some(now);
                self.status.stop_reason = Some(failure.reason().into());
                self.status.next_scan_at = None;
                return;
            }
            let multiplier = 1u64 << self.status.consecutive_failures.min(8);
            let delay = settings
                .effective_scan_interval(minimum)
                .saturating_mul(multiplier)
                .min(86400)
                .max(minimum);
            self.retry_after = Some(now + Duration::seconds(delay as i64));
        } else {
            self.status.consecutive_failures = 0;
            self.status.last_success_at = Some(now);
            self.retry_after = None;
        }
        self.status.next_scan_at = self.due_at(settings, minimum);
    }
    pub fn cancel(&mut self, now: DateTime<Utc>) {
        self.status.last_finished_at = Some(now);
        self.status.running = false;
        self.status.next_scan_at = None;
    }
    pub fn resume(&mut self) {
        self.status.stop_reason = None;
        self.retry_after = None;
    }
}
