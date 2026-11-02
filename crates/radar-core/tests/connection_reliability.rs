use chrono::{Duration, Utc};
use radar_core::{
    apple::{self, ScanFailure},
    scan_health::AlertStatus,
    *,
};
use serde_json::{Value, json};

fn fixture() -> (Database, Vec<Target>) {
    let mut db = Database::memory().unwrap();
    let catalog = Catalog::load().unwrap();
    let products: Vec<_> = catalog
        .products
        .iter()
        .filter(|p| p.locale == "zh_CN")
        .take(2)
        .map(|p| p.id.clone())
        .collect();
    let stores: Vec<_> = catalog
        .stores
        .iter()
        .filter(|s| s.locale == "zh_CN")
        .take(3)
        .map(|s| s.id.clone())
        .collect();
    db.add_targets(&catalog, &products, &stores, "连接可靠性")
        .unwrap();
    let targets = db.targets(Source::Apple, Utc::now()).unwrap();
    (db, targets)
}

fn response(targets: &[Target]) -> Value {
    let batches = apple::batches(targets.to_vec());
    let stores: Vec<_> = batches
        .iter()
        .map(|batch| {
            let mut parts = serde_json::Map::new();
            for target in &batch.targets {
                parts.insert(
                    target.product.sku.clone(),
                    json!({"partNumber":target.product.sku,"pickupDisplay":"available"}),
                );
            }
            json!({"storeNumber":batch.store,"partsAvailability":parts})
        })
        .collect();
    json!({"head":{"status":200},"body":{"stores":stores}})
}

#[test]
fn a_multi_store_response_covers_exact_watched_skus_without_more_requests() {
    let (_, targets) = fixture();
    let batch = apple::batches(targets.clone()).remove(0);
    let raw = serde_json::to_vec(&response(&targets)).unwrap();
    assert_eq!(batch.parse(&raw).unwrap().len(), 2);
    let output = batch.parse_coverage(&raw, &targets).unwrap();
    assert_eq!(output.len(), 6);
    assert!(
        output
            .iter()
            .all(|o| o.state == StockState::InStock && o.source == Source::Apple)
    );
    let ids: std::collections::HashSet<_> = output.iter().map(|o| &o.target_id).collect();
    assert_eq!(ids.len(), 6);
    for target in &targets {
        assert!(ids.contains(&target.id));
    }
}

#[test]
fn coverage_never_invents_missing_nearby_stock_or_crosses_locale_enabled_or_requested_sku_scope() {
    let (_, mut targets) = fixture();
    let batch = apple::batches(targets.clone()).remove(0);
    let mut hidden = targets[0].clone();
    hidden.id = "disabled".into();
    hidden.enabled = false;
    targets.push(hidden);
    let mut foreign = targets[0].clone();
    foreign.id = "foreign".into();
    foreign.product.locale = "zh_HK".into();
    foreign.store.locale = "zh_HK".into();
    targets.push(foreign);
    let mut unrequested = targets[0].clone();
    unrequested.id = "other-sku".into();
    unrequested.product.sku = "NOT_REQUESTED".into();
    targets.push(unrequested);
    let single = response(&batch.targets);
    let output = batch
        .parse_coverage(&serde_json::to_vec(&single).unwrap(), &targets)
        .unwrap();
    assert_eq!(output.len(), 2);
    let missing = json!({"head":{"status":200},"body":{"stores":[]}});
    let output = batch
        .parse_coverage(&serde_json::to_vec(&missing).unwrap(), &targets)
        .unwrap();
    assert_eq!(output.len(), 2);
    assert!(output.iter().all(|o| o.state == StockState::Unknown));
    let complete = response(
        &targets
            .iter()
            .filter(|t| t.product.locale == batch.locale)
            .cloned()
            .collect::<Vec<_>>(),
    );
    let output = batch
        .parse_coverage(&serde_json::to_vec(&complete).unwrap(), &targets)
        .unwrap();
    assert_eq!(output.len(), 6);
}

#[test]
fn duplicate_nearby_store_and_conflicting_sku_cannot_become_valid_stock() {
    let (_, targets) = fixture();
    let batch = apple::batches(targets.clone()).remove(0);
    let mut value = response(&targets);
    let stores = value["body"]["stores"].as_array_mut().unwrap();
    let duplicate = stores
        .iter()
        .find(|s| s["storeNumber"] != batch.store)
        .unwrap()
        .clone();
    stores.push(duplicate);
    assert_eq!(
        batch
            .parse_coverage(&serde_json::to_vec(&value).unwrap(), &targets)
            .unwrap_err(),
        ScanFailure::Parse
    );
    let mut value = response(&targets);
    let store = value["body"]["stores"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|s| s["storeNumber"] != batch.store)
        .unwrap();
    let parts = store["partsAvailability"].as_object_mut().unwrap();
    parts.values_mut().next().unwrap()["partNumber"] = "WRONG".into();
    let output = batch
        .parse_coverage(&serde_json::to_vec(&value).unwrap(), &targets)
        .unwrap();
    assert!(
        output
            .iter()
            .any(|o| o.state == StockState::Unknown && o.reason.as_deref() == Some("PARSE_ERROR"))
    );
}

#[test]
fn planner_skips_already_covered_stores_and_keeps_fairness_after_restart() {
    let (_, mut targets) = fixture();
    let initial = apple::batches(targets.clone());
    assert_eq!(apple::next_batch_index(&initial, 2), Some(2));
    let first_store = initial[0].store.clone();
    for target in &mut targets {
        if target.store.id == first_store {
            target.observed_at = Some(Utc::now().to_rfc3339());
        }
    }
    let remaining = apple::batches(targets.clone());
    assert_ne!(apple::next_batch_index(&remaining, 0), Some(0));
    for target in &mut targets {
        target.observed_at = Some(Utc::now().to_rfc3339());
    }
    let oldest_id = targets
        .iter()
        .find(|t| t.store.id == initial[2].store)
        .unwrap()
        .id
        .clone();
    targets
        .iter_mut()
        .find(|t| t.id == oldest_id)
        .unwrap()
        .observed_at = Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
    assert_eq!(
        apple::next_batch_index(&apple::batches(targets), 0),
        Some(2)
    );
    assert_eq!(apple::next_batch_index(&[], 0), None);
}

#[test]
fn protected_profile_limits_normal_and_immediate_requests_without_rewriting_user_interval() {
    let (mut db, _) = fixture();
    let settings = Settings {
        mode: Source::Apple,
        apple_personal_access: true,
        scan_running: true,
        scan_interval_seconds: 30,
        scan_connection_protection: true,
        ..Settings::default()
    };
    db.save_settings(&settings).unwrap();
    let base = Utc::now() - Duration::hours(2);
    let mut count = 0;
    for second in 0..3600 {
        if db
            .reserve_scan_request(base + Duration::seconds(second), 5, true)
            .unwrap()
        {
            count += 1;
        }
    }
    assert_eq!(count, 20);
    assert_eq!(db.settings().unwrap().scan_interval_seconds, 30);
    let file = tempfile::NamedTempFile::new().unwrap();
    db.backup(file.path()).unwrap();
    let mut restarted = Database::open(file.path()).unwrap();
    assert!(
        !restarted
            .reserve_scan_request(base + Duration::seconds(3599), 5, true)
            .unwrap()
    );
    assert!(
        restarted
            .reserve_scan_request(base + Duration::seconds(3600), 5, true)
            .unwrap()
    );
}

fn alerted() -> Settings {
    Settings {
        mode: Source::Apple,
        feishu_enabled: true,
        scan_failure_notifications: true,
        ..Settings::default()
    }
}

#[test]
fn source_alert_is_opt_in_one_per_incident_and_claimed_only_once() {
    let at = Utc::now();
    let mut settings = alerted();
    settings.queue_scan_alert(at, ScanFailure::Network, None);
    assert!(settings.scan_alert.is_none());
    settings.queue_scan_alert(at, ScanFailure::Blocked, Some(541));
    let id = settings.scan_alert.as_ref().unwrap().id.clone();
    settings.queue_scan_alert(at, ScanFailure::Blocked, Some(541));
    assert_eq!(settings.scan_alert.as_ref().unwrap().id, id);
    let claimed = settings.claim_scan_alert().unwrap();
    assert!(settings.claim_scan_alert().is_none());
    assert!(claimed.message().1.contains("HTTP 541"));
    settings.finish_scan_alert("old-id", Ok(()));
    assert_eq!(
        settings.scan_alert.as_ref().unwrap().status,
        AlertStatus::Sending
    );
    settings.finish_scan_alert(&id, Ok(()));
    settings.queue_scan_alert(at, ScanFailure::Blocked, Some(541));
    assert!(settings.claim_scan_alert().is_none());
    settings.queue_scan_alert(at + Duration::minutes(10), ScanFailure::Blocked, Some(541));
    assert!(settings.claim_scan_alert().is_some());
    let mut disabled = alerted();
    disabled.scan_failure_notifications = false;
    disabled.queue_scan_alert(at, ScanFailure::Blocked, Some(541));
    assert!(disabled.scan_alert.is_none());
    let mut mock = alerted();
    mock.mode = Source::Mock;
    mock.queue_scan_alert(at, ScanFailure::Blocked, Some(541));
    assert!(mock.scan_alert.is_none());
}

#[test]
fn restart_restore_and_channel_disable_do_not_replay_old_source_alerts() {
    let (db, _) = fixture();
    let mut settings = alerted();
    settings.queue_scan_alert(Utc::now(), ScanFailure::Blocked, Some(541));
    db.save_settings(&settings).unwrap();
    db.cancel_pending_feishu().unwrap();
    assert_eq!(
        db.settings().unwrap().scan_alert.unwrap().status,
        AlertStatus::Cancelled
    );
    settings.claim_scan_alert().unwrap();
    db.save_settings(&settings).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    db.backup(file.path()).unwrap();
    let mut restarted = Database::open(file.path()).unwrap();
    assert_eq!(
        restarted.settings().unwrap().scan_alert.unwrap().status,
        AlertStatus::Uncertain
    );
    restarted.restore(file.path()).unwrap();
    let settings = restarted.settings().unwrap();
    assert!(!settings.feishu_enabled);
    assert_eq!(settings.scan_alert.unwrap().status, AlertStatus::Uncertain);
}

#[test]
fn settings_edits_cannot_forge_alert_ack_or_transport_diagnostics() {
    let mut before = alerted();
    before.queue_scan_alert(Utc::now(), ScanFailure::Blocked, Some(541));
    before.scan_last_http_version = Some("HTTP/2".into());
    before.scan_last_duration_ms = Some(250);
    let mut incoming = before.clone();
    incoming.scan_alert.as_mut().unwrap().status = AlertStatus::Sent;
    incoming.scan_last_http_version = Some("HTTP/3".into());
    incoming.scan_last_duration_ms = Some(1);
    let saved = incoming.user_update(&before, true).unwrap();
    assert_eq!(saved.scan_alert.unwrap().status, AlertStatus::Pending);
    assert_eq!(saved.scan_last_http_version.as_deref(), Some("HTTP/2"));
    assert_eq!(saved.scan_last_duration_ms, Some(250));
}

#[test]
fn explicit_recovery_or_pause_cancels_a_pending_stale_failure_notice() {
    let mut settings = alerted();
    settings.queue_scan_alert(Utc::now(), ScanFailure::Blocked, Some(541));
    settings.set_scanning_at(true, true, Utc::now()).unwrap();
    assert_eq!(
        settings.scan_alert.as_ref().unwrap().status,
        AlertStatus::Cancelled
    );
    settings.queue_scan_alert(Utc::now(), ScanFailure::Blocked, Some(541));
    let mut paused = settings.clone();
    paused.paused = true;
    let result = paused.user_update(&settings, true).unwrap();
    assert_eq!(
        result.scan_alert.as_ref().unwrap().status,
        AlertStatus::Cancelled
    );
}

#[test]
fn source_alert_claim_is_atomic_across_connections_and_ack_cannot_overwrite_new_incident() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("alerts.sqlite");
    let first = Database::open(&path).unwrap();
    let second = Database::open(&path).unwrap();
    let mut settings = alerted();
    settings.queue_scan_alert(Utc::now(), ScanFailure::Blocked, Some(541));
    let original_id = settings.scan_alert.as_ref().unwrap().id.clone();
    first.save_settings(&settings).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [first, second]
        .into_iter()
        .map(|db| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                db.claim_scan_alert().unwrap().is_some()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|claimed| *claimed)
            .count(),
        1
    );
    let database = Database::open(&path).unwrap();
    let mut next = alerted();
    next.queue_scan_alert(
        Utc::now() + Duration::seconds(1),
        ScanFailure::Blocked,
        Some(541),
    );
    let next_id = next.scan_alert.as_ref().unwrap().id.clone();
    database.save_settings(&next).unwrap();
    database.finish_scan_alert(&original_id, Ok(())).unwrap();
    assert_eq!(database.settings().unwrap().scan_alert.unwrap().id, next_id);
    assert_eq!(
        database.settings().unwrap().scan_alert.unwrap().status,
        AlertStatus::Pending
    );
    assert!(database.claim_scan_alert().unwrap().is_some());
    database.finish_scan_alert(&next_id, Ok(())).unwrap();
    assert_eq!(
        database.settings().unwrap().scan_alert.unwrap().status,
        AlertStatus::Sent
    );
}
