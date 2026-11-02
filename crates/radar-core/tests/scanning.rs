use chrono::{Duration, Utc};
use radar_core::{
    apple::{self, AppleAuthorization, ScanFailure},
    scanner::Scanner,
    *,
};
use rusqlite::params;
use serde_json::json;

fn fixture() -> (Database, String) {
    let mut d = Database::memory().unwrap();
    let c = Catalog::load().unwrap();
    let products: Vec<_> = c
        .products
        .iter()
        .filter(|p| p.locale == "zh_CN")
        .take(2)
        .map(|p| p.id.clone())
        .collect();
    let stores: Vec<_> = c
        .stores
        .iter()
        .filter(|s| s.locale == "zh_CN")
        .take(3)
        .map(|s| s.id.clone())
        .collect();
    d.add_targets(&c, &products, &stores, "扫描测试").unwrap();
    let id = d.targets(Source::Manual, Utc::now()).unwrap()[0].id.clone();
    (d, id)
}
fn settings() -> Settings {
    Settings {
        mode: Source::Apple,
        scan_running: true,
        scan_interval_seconds: 30,
        ..Settings::default()
    }
}
fn batch() -> apple::AppleBatch {
    let (d, _) = fixture();
    apple::batches(d.targets(Source::Apple, Utc::now()).unwrap()).remove(0)
}
fn response(batch: &apple::AppleBatch, display: &str) -> serde_json::Value {
    let mut parts = serde_json::Map::new();
    for t in &batch.targets {
        parts.insert(
            t.product.sku.clone(),
            json!({"partNumber":t.product.sku,"pickupDisplay":display}),
        );
    }
    json!({"head":{"status":"200"},"body":{"stores":[{"storeNumber":batch.store,"partsAvailability":parts}]}})
}

#[test]
fn batching_deduplicates_and_never_crosses_store_or_region() {
    let (d, _) = fixture();
    let mut targets = d.targets(Source::Apple, Utc::now()).unwrap();
    targets.push(targets[0].clone());
    let batches = apple::batches(targets);
    assert_eq!(batches.len(), 3);
    assert_eq!(batches.iter().map(|b| b.targets.len()).sum::<usize>(), 6);
    for batch in batches {
        let url = batch.request_url().unwrap();
        assert_eq!(url.host_str(), Some("www.apple.com.cn"));
        assert!(
            batch
                .targets
                .iter()
                .all(|t| t.store.id == batch.store && t.product.locale == batch.locale)
        );
        let pairs: Vec<_> = url.query_pairs().collect();
        assert!(
            pairs
                .iter()
                .any(|(k, v)| k == "parts.0" && v == &batch.targets[0].product.sku)
        );
    }
}
#[test]
fn hong_kong_request_uses_its_own_endpoint_and_skus() {
    let mut d = Database::memory().unwrap();
    let c = Catalog::load().unwrap();
    let p = c.products.iter().find(|p| p.locale == "zh_HK").unwrap();
    let s = c.stores.iter().find(|s| s.locale == "zh_HK").unwrap();
    d.add_targets(
        &c,
        std::slice::from_ref(&p.id),
        std::slice::from_ref(&s.id),
        "HK",
    )
    .unwrap();
    let b = apple::batches(d.targets(Source::Apple, Utc::now()).unwrap()).remove(0);
    let url = b.request_url().unwrap();
    assert_eq!(url.host_str(), Some("www.apple.com"));
    assert_eq!(url.path(), "/hk-zh/shop/retail/pickup-message");
}
#[test]
fn valid_response_maps_only_explicit_available_and_unavailable() {
    let b = batch();
    for (display, state) in [
        ("available", StockState::InStock),
        ("unavailable", StockState::OutOfStock),
        ("ineligible", StockState::Unknown),
        ("future-value", StockState::Unknown),
    ] {
        let results = b
            .parse(&serde_json::to_vec(&response(&b, display)).unwrap())
            .unwrap();
        assert!(
            results
                .iter()
                .all(|r| r.state == state && r.source == Source::Apple)
        );
    }
}
#[test]
fn wrong_store_missing_sku_and_conflicting_sku_never_mean_out_of_stock() {
    let b = batch();
    let mut v = response(&b, "available");
    v["body"]["stores"][0]["storeNumber"] = json!("R_OTHER");
    assert!(
        b.parse(&serde_json::to_vec(&v).unwrap())
            .unwrap()
            .iter()
            .all(|r| r.state == StockState::Unknown && r.reason.as_deref() == Some("NO_MATCH"))
    );
    let mut v = response(&b, "available");
    v["body"]["stores"][0]["partsAvailability"] = json!({});
    assert!(
        b.parse(&serde_json::to_vec(&v).unwrap())
            .unwrap()
            .iter()
            .all(|r| r.state == StockState::Unknown)
    );
    let mut v = response(&b, "available");
    v["body"]["stores"][0]["partsAvailability"][&b.targets[0].product.sku]["partNumber"] =
        json!("WRONG");
    let results = b.parse(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(results[0].reason.as_deref(), Some("PARSE_ERROR"));
    assert_eq!(results[0].state, StockState::Unknown);
}
#[test]
fn rejection_envelope_html_and_duplicate_store_are_not_successful_stock_results() {
    let b = batch();
    assert_eq!(
        b.parse(b"<html>Access denied</html>").unwrap_err(),
        ScanFailure::Blocked
    );
    for (status, failure) in [
        (401, ScanFailure::Unauthorized),
        (403, ScanFailure::Blocked),
        (429, ScanFailure::RateLimited),
        (541, ScanFailure::Blocked),
        (500, ScanFailure::Service),
    ] {
        let mut v = response(&b, "available");
        v["head"]["status"] = json!(status);
        assert_eq!(
            b.parse(&serde_json::to_vec(&v).unwrap()).unwrap_err(),
            failure
        );
    }
    let mut v = response(&b, "available");
    v["body"]["errorMessage"] = json!("service failure");
    assert!(b.parse(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v = response(&b, "available");
    let duplicate = v["body"]["stores"][0].clone();
    v["body"]["stores"].as_array_mut().unwrap().push(duplicate);
    assert!(b.parse(&serde_json::to_vec(&v).unwrap()).is_err());
}
#[test]
fn registered_permission_must_be_scoped_current_and_have_a_rate_budget() {
    let now = Utc::now();
    let mut grant = AppleAuthorization {
        mechanism: "apple-pickup-message-v1".into(),
        permission_reference: "verified document reference".into(),
        allowed_locales: vec!["zh_CN".into()],
        minimum_interval_seconds: 300,
        expires_at: now + Duration::days(1),
    };
    assert!(grant.validate(now).is_ok());
    grant.expires_at = now;
    assert!(grant.validate(now).is_err());
    grant.expires_at = now + Duration::days(1);
    grant.allowed_locales = vec!["en_US".into()];
    assert!(grant.validate(now).is_err());
}
#[test]
fn interval_uses_finish_time_and_permission_minimum_and_never_overlaps() {
    let mut scan = Scanner::default();
    let s = settings();
    let now = Utc::now();
    assert!(!scan.begin(&s, 300, false, now));
    assert!(scan.begin(&s, 300, true, now));
    assert!(!scan.begin(&s, 300, true, now + Duration::seconds(500)));
    scan.finish(&s, 300, now + Duration::seconds(8), None, 6);
    assert!(!scan.begin(&s, 300, true, now + Duration::seconds(307)));
    assert!(scan.begin(&s, 300, true, now + Duration::seconds(308)));
}
#[test]
fn interval_changes_take_effect_and_wake_does_not_catch_up() {
    let mut scan = Scanner::default();
    let mut s = settings();
    let now = Utc::now();
    assert!(scan.begin(&s, 5, true, now));
    scan.finish(&s, 5, now, None, 1);
    s.scan_interval_seconds = 60;
    assert!(!scan.begin(&s, 5, true, now + Duration::seconds(59)));
    assert!(scan.begin(&s, 5, true, now + Duration::hours(1)));
    assert!(!scan.begin(&s, 5, true, now + Duration::hours(1)));
    scan.finish(&s, 5, now + Duration::hours(1), None, 1);
    assert_eq!(
        scan.status.next_scan_at,
        Some(now + Duration::hours(1) + Duration::seconds(60))
    );
}
#[test]
fn network_backoff_and_explicit_resume_after_rejection_are_enforced() {
    let mut scan = Scanner::default();
    let s = settings();
    let now = Utc::now();
    assert!(scan.begin(&s, 5, true, now));
    scan.finish(&s, 5, now, Some(ScanFailure::Timeout), 1);
    assert_eq!(scan.status.next_scan_at, Some(now + Duration::seconds(60)));
    assert!(!scan.begin(&s, 5, true, now + Duration::seconds(30)));
    assert!(scan.begin(&s, 5, true, now + Duration::seconds(60)));
    scan.finish(
        &s,
        5,
        now + Duration::seconds(60),
        Some(ScanFailure::RateLimited),
        1,
    );
    assert_eq!(scan.status.stop_reason.as_deref(), Some("RATE_LIMITED"));
    assert!(!scan.begin(&s, 5, true, now + Duration::days(1)));
    scan.resume();
    assert!(scan.begin(&s, 5, true, now + Duration::days(1)));
}
#[test]
fn pause_source_switch_and_disabled_scheduling_prevent_checks() {
    let mut scan = Scanner::default();
    let now = Utc::now();
    let mut s = settings();
    s.paused = true;
    assert!(!scan.begin(&s, 5, true, now));
    s.paused = false;
    s.mode = Source::Mock;
    assert!(!scan.begin(&s, 5, true, now));
    s.mode = Source::Apple;
    s.scan_running = false;
    assert!(!scan.begin(&s, 5, true, now));
}
#[test]
fn apple_history_and_notifications_are_independent_from_manual_and_mock() {
    let (mut d, id) = fixture();
    let now = Utc::now();
    let mut s = settings();
    s.local_notifications = true;
    d.save_settings(&s).unwrap();
    let input = |state| ObservationInput {
        id: uuid::Uuid::new_v4().to_string(),
        target_id: id.clone(),
        source: Source::Apple,
        state,
        reason: None,
    };
    d.record(&input(StockState::OutOfStock), now).unwrap();
    assert_eq!(
        d.record(&input(StockState::InStock), now)
            .unwrap()
            .as_deref(),
        Some("RESTOCK_DETECTED")
    );
    assert_eq!(d.summary(Source::Apple, 7, None, 480).unwrap().valid, 2);
    assert_eq!(d.summary(Source::Manual, 7, None, 480).unwrap().total, 0);
    assert_eq!(d.summary(Source::Mock, 7, None, 480).unwrap().total, 0);
    let deliveries = d.deliveries().unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].source, Source::Apple);
    assert!(deliveries[0].body.contains("Apple 商店查询"));
}

fn make_v1_backup(path: &std::path::Path) {
    let (mut d, id) = fixture();
    let s = Settings {
        local_notifications: true,
        ..Settings::default()
    };
    d.save_settings(&s).unwrap();
    d.record(
        &ObservationInput {
            id: "old-observation".into(),
            target_id: id,
            source: Source::Manual,
            state: StockState::InStock,
            reason: None,
        },
        Utc::now(),
    )
    .unwrap();
    let mut value = serde_json::to_value(s).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("scan_last_request_at");
    for field in [
        "apple_personal_access",
        "scan_stop_reason",
        "scan_invalidated_at",
    ] {
        value.as_object_mut().unwrap().remove(field);
    }
    value.as_object_mut().unwrap().remove("scan_running");
    value
        .as_object_mut()
        .unwrap()
        .remove("scan_interval_seconds");
    d.connection
        .execute("UPDATE settings SET json=?1", params![value.to_string()])
        .unwrap();
    d.connection.execute_batch("PRAGMA foreign_keys=OFF; BEGIN;
        CREATE TABLE observations_v1(id TEXT PRIMARY KEY,target_id TEXT NOT NULL REFERENCES targets(id),source TEXT NOT NULL CHECK(source IN ('manual','mock')),state TEXT NOT NULL CHECK(state IN ('IN_STOCK','OUT_OF_STOCK','UNKNOWN')),observed_at TEXT NOT NULL,reason TEXT);
        INSERT INTO observations_v1 SELECT * FROM observations;DROP TABLE observations;ALTER TABLE observations_v1 RENAME TO observations;
        CREATE INDEX observations_target_time ON observations(target_id,source,observed_at DESC);UPDATE schema_version SET version=1;PRAGMA user_version=1;COMMIT;").unwrap();
    d.backup(path).unwrap();
}
#[test]
fn v1_database_upgrade_keeps_observations_events_and_legacy_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("radar.sqlite");
    make_v1_backup(&path);
    let d = Database::open(&path).unwrap();
    let version: i64 = d
        .connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
    assert_eq!(d.settings().unwrap().scan_interval_seconds, 300);
    assert!(!d.settings().unwrap().scan_running);
    assert_eq!(d.history(Source::Manual, 7, None).unwrap().len(), 1);
    assert_eq!(d.deliveries().unwrap().len(), 1);
    assert!(
        std::fs::read_dir(dir.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("pre-v3-"))
    );
}
#[test]
fn v1_backup_can_be_restored_and_then_accept_apple_samples() {
    let file = tempfile::NamedTempFile::new().unwrap();
    make_v1_backup(file.path());
    let mut d = Database::memory().unwrap();
    d.restore(file.path()).unwrap();
    let id = d.targets(Source::Apple, Utc::now()).unwrap()[0].id.clone();
    d.save_settings(&settings()).unwrap();
    d.record(
        &ObservationInput {
            id: "new-apple".into(),
            target_id: id,
            source: Source::Apple,
            state: StockState::InStock,
            reason: None,
        },
        Utc::now(),
    )
    .unwrap();
    assert_eq!(d.history(Source::Apple, 7, None).unwrap().len(), 1);
    assert_eq!(d.history(Source::Manual, 7, None).unwrap().len(), 1);
}

#[test]
fn transport_cannot_construct_an_authorized_request_without_current_scoped_permission() {
    let b = batch();
    let now = Utc::now();
    assert!(apple::authorized_request(None, &b, now).is_err());
    let mut grant = AppleAuthorization {
        mechanism: "apple-pickup-message-v1".into(),
        permission_reference: "offline-test-permission-reference".into(),
        allowed_locales: vec!["zh_CN".into()],
        minimum_interval_seconds: 300,
        expires_at: now + Duration::hours(1),
    };
    assert!(apple::authorized_request(Some(&grant), &b, now).is_ok());
    grant.allowed_locales = vec!["zh_HK".into()];
    assert!(apple::authorized_request(Some(&grant), &b, now).is_err());
    grant.allowed_locales = vec!["zh_CN".into()];
    grant.expires_at = now;
    assert!(apple::authorized_request(Some(&grant), &b, now).is_err());
}

#[test]
fn cancelled_requests_and_process_restart_keep_the_interval_budget() {
    let now = Utc::now();
    let mut s = settings();
    let mut scanner = Scanner::default();
    assert!(scanner.begin(&s, 30, true, now));
    scanner.cancel(now + Duration::seconds(2));
    assert!(!scanner.begin(&s, 30, true, now + Duration::seconds(31)));
    s.scan_last_request_at = Some(now + Duration::seconds(2));
    let mut restarted = Scanner::default();
    assert!(!restarted.begin(&s, 30, true, now + Duration::seconds(31)));
    assert!(!restarted.begin(&s, 30, true, now + Duration::seconds(32)));
    assert!(restarted.begin(&s, 30, true, now + Duration::seconds(62)));
}

#[test]
fn resuming_a_blocked_source_does_not_restore_trust_in_old_samples() {
    let now = Utc::now();
    let s = settings();
    let mut scan = Scanner::default();
    scan.begin(&s, 5, true, now);
    scan.finish(
        &s,
        5,
        now + Duration::seconds(2),
        Some(ScanFailure::Blocked),
        1,
    );
    assert_eq!(scan.status.invalidated_at, Some(now + Duration::seconds(2)));
    scan.resume();
    assert!(scan.status.stop_reason.is_none());
    assert_eq!(scan.status.invalidated_at, Some(now + Duration::seconds(2)));
    assert!(scan.begin(&s, 5, true, now + Duration::seconds(40)));
    scan.finish(&s, 5, now + Duration::seconds(41), None, 1);
    assert_eq!(scan.status.invalidated_at, Some(now + Duration::seconds(2)));
    assert_eq!(
        scan.status.last_success_at,
        Some(now + Duration::seconds(41))
    );
}

#[test]
fn personal_opt_in_is_distinct_from_provider_authorization_and_defaults_off() {
    let now = Utc::now();
    let batch = batch();
    let defaults = Settings::default();
    assert!(!defaults.apple_personal_access);
    let disabled = apple::AppleAccess::configured(defaults.apple_personal_access, None);
    assert!(apple::accessible_request(&disabled, &batch, now).is_err());
    let personal = apple::AppleAccess::configured(true, None);
    assert!(matches!(personal, apple::AppleAccess::Personal));
    assert_eq!(personal.kind(), "personal");
    assert_eq!(personal.minimum_interval(), 5);
    assert!(apple::accessible_request(&personal, &batch, now).is_ok());
}

#[test]
fn editing_an_interval_cannot_resume_a_stopped_source_or_erase_its_budget() {
    let now = Utc::now();
    let previous = Settings {
        mode: Source::Apple,
        apple_personal_access: true,
        scan_running: false,
        scan_last_request_at: Some(now),
        scan_stop_reason: Some("BLOCKED".into()),
        scan_invalidated_at: Some(now),
        ..Settings::default()
    };
    let incoming = Settings {
        mode: Source::Apple,
        apple_personal_access: true,
        scan_running: true,
        scan_interval_seconds: 60,
        ..Settings::default()
    };
    let updated = incoming.user_update(&previous, true).unwrap();
    assert!(!updated.scan_running);
    assert_eq!(updated.scan_interval_seconds, 60);
    assert_eq!(updated.scan_stop_reason.as_deref(), Some("BLOCKED"));
    assert_eq!(updated.scan_last_request_at, Some(now));
    assert_eq!(updated.scan_invalidated_at, Some(now));
}

#[test]
fn explicit_scan_start_requires_apple_mode_and_enabled_access() {
    let mut settings = Settings::default();
    assert!(settings.set_scanning(true, true).is_err());
    settings.mode = Source::Apple;
    assert!(settings.set_scanning(true, false).is_err());
    settings.scan_stop_reason = Some("BLOCKED".into());
    settings.set_scanning(true, true).unwrap();
    assert!(settings.scan_running);
    assert!(settings.scan_stop_reason.is_none());
    settings.set_scanning(false, false).unwrap();
    assert!(!settings.scan_running);
}

#[test]
fn personal_mode_does_not_override_a_registered_providers_scope_or_budget() {
    let now = Utc::now();
    let grant = AppleAuthorization {
        mechanism: "apple-pickup-message-v1".into(),
        permission_reference: "offline-verified-reference".into(),
        allowed_locales: vec!["zh_CN".into()],
        minimum_interval_seconds: 300,
        expires_at: now + Duration::hours(1),
    };
    let access = apple::AppleAccess::configured(true, Some(&grant));
    assert!(matches!(access, apple::AppleAccess::Licensed(_)));
    assert_eq!(access.minimum_interval(), 300);
    assert_eq!(access.locales(), vec!["zh_CN"]);
}

#[test]
fn personal_preference_and_source_stop_are_preserved_on_restart() {
    let (d, _) = fixture();
    let now = Utc::now();
    let settings = Settings {
        mode: Source::Apple,
        apple_personal_access: true,
        scan_running: false,
        scan_stop_reason: Some("BLOCKED".into()),
        scan_invalidated_at: Some(now),
        scan_last_http_status: Some(541),
        ..Settings::default()
    };
    d.save_settings(&settings).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    d.backup(file.path()).unwrap();
    let reopened = Database::open(file.path()).unwrap();
    let settings = reopened.settings().unwrap();
    assert!(settings.apple_personal_access);
    assert!(!settings.scan_running);
    assert_eq!(settings.scan_stop_reason.as_deref(), Some("BLOCKED"));
    assert_eq!(settings.scan_invalidated_at, Some(now));
    assert_eq!(settings.scan_last_http_status, Some(541));
}
