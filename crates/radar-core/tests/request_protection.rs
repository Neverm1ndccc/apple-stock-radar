use chrono::{DateTime, Duration, Utc};
use radar_core::{
    apple::ScanFailure,
    request_budget::{RequestBudget, retry_after},
    *,
};

fn configured() -> Database {
    let d = Database::memory().unwrap();
    d.save_settings(&Settings {
        mode: Source::Apple,
        apple_personal_access: true,
        scan_running: true,
        scan_interval_seconds: 10,
        ..Settings::default()
    })
    .unwrap();
    d
}

#[test]
fn ten_second_polling_is_capped_by_cumulative_budget_instead_of_repeating_53_times() {
    let mut d = configured();
    let base = Utc::now() - Duration::minutes(20);
    let mut sent = Vec::new();
    for second in (0..=600).step_by(10) {
        if d.reserve_scan_request(base + Duration::seconds(second), 5, false)
            .unwrap()
        {
            sent.push(second);
        }
    }
    assert_eq!(&sent[..7], &[0, 10, 20, 30, 40, 50, 60]);
    assert_eq!(sent.len(), 16);
    assert_eq!(sent[7], 120);
    assert_eq!(d.settings().unwrap().scan_interval_seconds, 10);
}

#[test]
fn consumed_credits_survive_process_restart_settings_edits_and_immediate_scan() {
    let mut d = configured();
    let base = Utc::now() - Duration::minutes(20);
    for second in (0..=50).step_by(10) {
        assert!(
            d.reserve_scan_request(base + Duration::seconds(second), 5, false)
                .unwrap()
        );
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    d.backup(file.path()).unwrap();
    let mut d = Database::open(file.path()).unwrap();
    let previous = d.settings().unwrap();
    let incoming = Settings {
        scan_budget: Some(RequestBudget::full(base)),
        scan_interval_seconds: 5,
        ..previous.clone()
    };
    d.save_settings(&incoming.user_update(&previous, true).unwrap())
        .unwrap();
    assert!(
        !d.reserve_scan_request(base + Duration::seconds(55), 5, true)
            .unwrap()
    );
    assert!(
        d.reserve_scan_request(base + Duration::seconds(60), 5, true)
            .unwrap()
    );
}

#[test]
fn concurrent_database_reservations_cannot_spend_the_same_request_slot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("radar.sqlite");
    let mut a = Database::open(&path).unwrap();
    a.save_settings(&configured().settings().unwrap()).unwrap();
    let mut b = Database::open(&path).unwrap();
    let at = Utc::now() - Duration::minutes(1);
    let barrier = std::sync::Barrier::new(2);
    let count = std::thread::scope(|scope| {
        let barrier = &barrier;
        let one = scope.spawn(move || {
            barrier.wait();
            a.reserve_scan_request(at, 5, false).unwrap()
        });
        let two = scope.spawn(move || {
            barrier.wait();
            b.reserve_scan_request(at, 5, false).unwrap()
        });
        usize::from(one.join().unwrap()) + usize::from(two.join().unwrap())
    });
    assert_eq!(count, 1);
}

#[test]
fn restoring_an_old_full_budget_cannot_replenish_live_credits_or_remove_a_block() {
    let mut d = configured();
    let base = Utc::now() - Duration::seconds(50);
    let mut s = d.settings().unwrap();
    s.scan_budget = Some(RequestBudget::full(base));
    d.save_settings(&s).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    d.backup(file.path()).unwrap();
    for second in (0..=50).step_by(10) {
        assert!(
            d.reserve_scan_request(base + Duration::seconds(second), 5, false)
                .unwrap()
        );
    }
    let mut s = d.settings().unwrap();
    s.apply_scan_result(Utc::now(), 5, Some(ScanFailure::Blocked), None);
    d.save_settings(&s).unwrap();
    d.restore(file.path()).unwrap();
    let s = d.settings().unwrap();
    assert_eq!(s.request_budget(Utc::now()).remaining(Utc::now()), 0);
    assert_eq!(s.scan_stop_reason.as_deref(), Some("BLOCKED"));
    assert!(!s.scan_running);
    assert!(s.resume_after().unwrap() > Utc::now());
}

#[test]
fn rejection_cooldown_escalates_and_does_not_reenable_automatically() {
    let base = Utc::now() - Duration::hours(2);
    let mut s = configured().settings().unwrap();
    s.apply_scan_result(base, 5, Some(ScanFailure::Blocked), None);
    assert!(!s.scan_running);
    assert_eq!(s.resume_after(), Some(base + Duration::minutes(5)));
    assert!(
        s.set_scanning_at(true, true, base + Duration::minutes(4))
            .is_err()
    );
    s.set_scanning_at(true, true, base + Duration::minutes(5))
        .unwrap();
    s.apply_scan_result(
        base + Duration::minutes(6),
        5,
        Some(ScanFailure::Blocked),
        None,
    );
    assert_eq!(s.resume_after(), Some(base + Duration::minutes(16)));
    assert!(!s.scan_running);
    assert!(
        s.set_scanning_at(true, true, base + Duration::minutes(15))
            .is_err()
    );
}

#[test]
fn retry_after_accepts_delta_and_http_date_without_shortening_server_waits() {
    let at = DateTime::parse_from_rfc3339("2026-10-25T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(retry_after("120", at), Some(at + Duration::seconds(120)));
    assert_eq!(
        retry_after("Sun, 25 Oct 2026 00:02:00 GMT", at),
        Some(at + Duration::seconds(120))
    );
    assert_eq!(
        retry_after("999999999", at),
        Some(at + Duration::seconds(999999999))
    );
    assert_eq!(retry_after("-1", at), None);
    assert_eq!(retry_after("invalid", at), None);
    let mut s = configured().settings().unwrap();
    s.apply_scan_result(
        at,
        5,
        Some(ScanFailure::RateLimited),
        retry_after("1800", at),
    );
    assert_eq!(s.resume_after(), Some(at + Duration::minutes(30)));
    assert!(!s.scan_running);
}

#[test]
fn temporary_service_errors_back_off_but_real_rejections_stop() {
    assert_eq!(ScanFailure::from_http(503), ScanFailure::Service);
    assert!(!ScanFailure::Service.stops_source());
    assert!(ScanFailure::from_http(541).stops_source());
    let at = Utc::now();
    let mut s = configured().settings().unwrap();
    s.apply_scan_result(
        at,
        5,
        Some(ScanFailure::Service),
        Some(at + Duration::minutes(2)),
    );
    assert!(s.scan_running);
    assert!(s.scan_stop_reason.is_none());
    assert_eq!(s.scan_retry_after, Some(at + Duration::minutes(2)));
    s.apply_scan_result(at + Duration::minutes(2), 5, None, None);
    assert_eq!(s.scan_failure_count, 0);
    assert!(s.scan_retry_after.is_none());
}

#[test]
fn network_backoff_allows_stop_start_but_keeps_requests_waiting_after_restart() {
    let d = configured();
    let old_backup = tempfile::NamedTempFile::new().unwrap();
    d.backup(old_backup.path()).unwrap();
    let at = Utc::now() - Duration::hours(8);
    let mut settings = d.settings().unwrap();
    settings.scan_connection_protection = true;
    settings.scan_failure_count = 6;
    settings.apply_scan_result(at, 5, Some(ScanFailure::Network), None);
    let deadline = settings.scan_retry_after.unwrap();
    let budget = settings.request_budget(at);
    assert_eq!(deadline, at + Duration::seconds(180 * 128));
    settings.set_scanning_at(false, true, at).unwrap();
    assert!(settings.resume_after().is_none());
    settings.set_scanning_at(true, true, at).unwrap();
    assert!(settings.scan_running);
    assert_eq!(settings.scan_failure_count, 7);
    assert_eq!(settings.scan_retry_after, Some(deadline));
    assert_eq!(
        settings.request_budget(at).remaining(at),
        budget.remaining(at)
    );
    d.save_settings(&settings).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    d.backup(file.path()).unwrap();
    let mut d = Database::open(file.path()).unwrap();
    let settings = d.settings().unwrap();
    let mut scanner = radar_core::scanner::Scanner::default();
    scanner.resume();
    assert_eq!(scanner.due_at(&settings, 5), Some(deadline));
    assert!(!scanner.begin(&settings, 5, true, deadline - Duration::seconds(1)));
    assert!(
        !d.reserve_scan_request(deadline - Duration::seconds(1), 5, false)
            .unwrap()
    );
    assert!(
        !d.reserve_scan_request(deadline - Duration::seconds(1), 5, true)
            .unwrap()
    );
    assert!(scanner.begin(&settings, 5, true, deadline));
    assert!(d.reserve_scan_request(deadline, 5, false).unwrap());
    d.restore(old_backup.path()).unwrap();
    let restored = d.settings().unwrap();
    assert!(!restored.scan_running);
    assert!(restored.resume_after().is_none());
    assert_eq!(restored.scan_retry_after, Some(deadline));
    assert_eq!(restored.scan_failure_count, 7);
}

#[test]
fn source_stop_still_blocks_restart_until_server_retry_after() {
    let at = Utc::now() - Duration::seconds(1);
    let mut settings = configured().settings().unwrap();
    let deadline = at + Duration::hours(2);
    settings.apply_scan_result(at, 5, Some(ScanFailure::RateLimited), Some(deadline));
    settings.set_scanning_at(false, true, at).unwrap();
    assert_eq!(settings.resume_after(), Some(deadline));
    assert!(
        settings
            .set_scanning_at(true, true, deadline - Duration::seconds(1))
            .is_err()
    );
    assert!(!settings.scan_running);
    settings.set_scanning_at(true, true, deadline).unwrap();
    assert!(settings.scan_running);
    assert!(settings.resume_after().is_none());
    assert_eq!(settings.scan_retry_after, Some(deadline));
}

#[test]
fn time_rollback_does_not_mint_credits_and_idle_time_cannot_exceed_capacity() {
    let at = Utc::now();
    let mut b = RequestBudget::empty(at);
    assert!(!b.take(at - Duration::minutes(1)));
    let half = b.projected(at + Duration::seconds(30));
    let rollback = half.projected(at - Duration::seconds(30));
    b = rollback.projected(at + Duration::seconds(60));
    assert!(b.take(at + Duration::seconds(60)));
    assert!(!b.take(at + Duration::seconds(60)));
    assert_eq!(b.remaining(at + Duration::days(1)), 6);
}
