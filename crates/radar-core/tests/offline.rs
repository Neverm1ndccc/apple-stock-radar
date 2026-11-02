use chrono::{Duration, Utc};
use radar_core::*;
use rusqlite::{Connection, params};

fn fixture() -> (Database, Catalog, String) {
    let mut db = Database::memory().unwrap();
    let catalog = Catalog::load().unwrap();
    let p = catalog
        .products
        .iter()
        .find(|p| p.locale == "zh_CN")
        .unwrap();
    let store = catalog.stores.iter().find(|s| s.locale == "zh_CN").unwrap();
    db.add_targets(
        &catalog,
        std::slice::from_ref(&p.id),
        std::slice::from_ref(&store.id),
        "测试",
    )
    .unwrap();
    let id = db.targets(Source::Manual, Utc::now()).unwrap()[0]
        .id
        .clone();
    (db, catalog, id)
}
fn input(target: &str, state: StockState, source: Source) -> ObservationInput {
    ObservationInput {
        id: uuid::Uuid::new_v4().to_string(),
        target_id: target.into(),
        state,
        source,
        reason: (state == StockState::Unknown).then(|| "TIMEOUT".into()),
    }
}
fn alerts(db: &Database) {
    db.save_settings(&Settings {
        local_notifications: true,
        bark_enabled: true,
        ..Settings::default()
    })
    .unwrap();
}

#[test]
fn offline_catalog_has_region_scoped_skus_and_trusted_urls() {
    let catalog = Catalog::load().unwrap();
    for locale in ["zh_CN", "zh_HK"] {
        assert!(catalog.products.iter().any(|p| p.locale == locale));
        assert!(catalog.stores.iter().any(|s| s.locale == locale));
    }
    for p in &catalog.products {
        official_url(&p.official_url).unwrap();
    }
    let ids: std::collections::HashSet<_> = catalog.products.iter().map(|p| &p.id).collect();
    assert_eq!(ids.len(), catalog.products.len());
}
#[test]
fn two_skus_three_stores_create_six_unique_targets_and_shared_group_memberships() {
    let mut db = Database::memory().unwrap();
    let catalog = Catalog::load().unwrap();
    let ps: Vec<_> = catalog
        .products
        .iter()
        .filter(|p| p.locale == "zh_CN")
        .take(2)
        .map(|p| p.id.clone())
        .collect();
    let ss: Vec<_> = catalog
        .stores
        .iter()
        .filter(|s| s.locale == "zh_CN")
        .take(3)
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(db.add_targets(&catalog, &ps, &ss, "一组").unwrap().added, 6);
    let again = db.add_targets(&catalog, &ps, &ss, "另一组").unwrap();
    assert_eq!((again.added, again.duplicates), (0, 6));
    assert_eq!(db.targets(Source::Manual, Utc::now()).unwrap().len(), 6);
    assert!(db.groups().unwrap().iter().all(|g| g.target_ids.len() == 6));
    let hk = catalog.stores.iter().find(|s| s.locale == "zh_HK").unwrap();
    assert!(
        db.add_targets(&catalog, &ps, std::slice::from_ref(&hk.id), "跨区")
            .is_err()
    );
    assert_eq!(db.groups().unwrap().len(), 2);
}
#[test]
fn restock_is_idempotent_and_continuous_in_stock_does_not_renotify() {
    let (mut db, _, id) = fixture();
    alerts(&db);
    let at = Utc::now();
    db.record(&input(&id, StockState::OutOfStock, Source::Manual), at)
        .unwrap();
    let observed = input(&id, StockState::InStock, Source::Manual);
    assert_eq!(
        db.record(&observed, at).unwrap().as_deref(),
        Some("RESTOCK_DETECTED")
    );
    assert!(db.record(&observed, at).unwrap().is_none());
    assert!(
        db.record(&input(&id, StockState::InStock, Source::Manual), at)
            .unwrap()
            .is_none()
    );
    assert_eq!(db.deliveries().unwrap().len(), 2);
    let mut conflicting = observed.clone();
    conflicting.state = StockState::OutOfStock;
    assert!(db.record(&conflicting, at).is_err());
    let delivery = db.deliveries().unwrap().remove(0);
    assert!(db.claim_delivery(&delivery.id).unwrap());
    assert!(!db.claim_delivery(&delivery.id).unwrap());
    db.finish_delivery(&delivery.id, Err("离线模拟失败".into()))
        .unwrap();
    assert_eq!(
        db.deliveries()
            .unwrap()
            .iter()
            .filter(|d| d.status == "FAILED")
            .count(),
        1
    );
}
#[test]
fn failure_is_unknown_and_preserves_last_valid_sample_recovery_is_distinct() {
    let (mut db, _, id) = fixture();
    let at = Utc::now();
    db.record(&input(&id, StockState::InStock, Source::Manual), at)
        .unwrap();
    db.record(&input(&id, StockState::Unknown, Source::Manual), at)
        .unwrap();
    let t = db.targets(Source::Manual, at).unwrap().remove(0);
    assert_eq!(t.state, "UNKNOWN");
    assert_eq!(t.last_state.as_deref(), Some("IN_STOCK"));
    assert_eq!(
        db.record(&input(&id, StockState::InStock, Source::Manual), at)
            .unwrap()
            .as_deref(),
        Some("RECOVERED_IN_STOCK")
    );
    assert_eq!(
        db.targets(Source::Manual, at + Duration::minutes(11))
            .unwrap()[0]
            .state,
        "STALE"
    );
    assert_eq!(
        db.record(
            &input(&id, StockState::InStock, Source::Manual),
            at + Duration::minutes(11)
        )
        .unwrap()
        .as_deref(),
        Some("RECOVERED_IN_STOCK")
    );
}
#[test]
fn first_and_recovery_alert_settings_do_not_suppress_events_or_restock_alerts() {
    let (mut db, _, id) = fixture();
    db.save_settings(&Settings {
        local_notifications: true,
        first_alert: false,
        recovery_alert: false,
        ..Settings::default()
    })
    .unwrap();
    let at = Utc::now();
    assert_eq!(
        db.record(&input(&id, StockState::InStock, Source::Manual), at)
            .unwrap()
            .as_deref(),
        Some("FIRST_IN_STOCK")
    );
    assert!(db.deliveries().unwrap().is_empty());
    db.record(&input(&id, StockState::OutOfStock, Source::Manual), at)
        .unwrap();
    db.record(&input(&id, StockState::InStock, Source::Manual), at)
        .unwrap();
    assert_eq!(db.deliveries().unwrap().len(), 1);
}
#[test]
fn mock_data_never_changes_manual_history_or_creates_real_deliveries() {
    let (mut db, _, id) = fixture();
    alerts(&db);
    db.record(
        &input(&id, StockState::OutOfStock, Source::Manual),
        Utc::now(),
    )
    .unwrap();
    let mut s = db.settings().unwrap();
    s.mode = Source::Mock;
    db.save_settings(&s).unwrap();
    assert!(
        db.record(&input(&id, StockState::InStock, Source::Manual), Utc::now())
            .is_err()
    );
    for _ in 0..6 {
        assert_eq!(db.mock_tick().unwrap(), 1);
    }
    assert!(db.deliveries().unwrap().is_empty());
    assert_eq!(
        db.targets(Source::Manual, Utc::now()).unwrap()[0].state,
        "OUT_OF_STOCK"
    );
    let summary = db.summary(Source::Mock, 7, None, 480).unwrap();
    assert_eq!(
        (
            summary.total,
            summary.valid,
            summary.in_stock,
            summary.events
        ),
        (6, 5, 3, 2)
    );
    assert_eq!(db.history(Source::Manual, 7, None).unwrap().len(), 1);
    db.clear(Source::Mock).unwrap();
    assert_eq!(db.history(Source::Manual, 7, None).unwrap().len(), 1);
    assert!(db.history(Source::Mock, 7, None).unwrap().is_empty());
}
#[test]
fn unknown_is_excluded_from_denominator_and_heatmap_uses_requested_offset() {
    let (mut db, _, id) = fixture();
    let at = Utc::now()
        .date_naive()
        .and_hms_opt(23, 30, 0)
        .unwrap()
        .and_utc()
        - Duration::days(1);
    for state in [
        StockState::OutOfStock,
        StockState::InStock,
        StockState::Unknown,
    ] {
        db.record(&input(&id, state, Source::Manual), at).unwrap();
    }
    let summary = db.summary(Source::Manual, 7, Some(&id), 480).unwrap();
    assert_eq!((summary.total, summary.valid, summary.in_stock), (3, 2, 1));
    assert_eq!(summary.cells.len(), 1);
    assert_eq!(summary.cells[0].hour, 7);
    assert_eq!(
        summary.cells[0].day,
        (at + Duration::hours(8)).format("%Y-%m-%d").to_string()
    );
}
#[test]
fn pause_archive_and_validation_prevent_accidental_observations_and_pushes() {
    let (mut db, _, id) = fixture();
    alerts(&db);
    let at = Utc::now();
    db.record(&input(&id, StockState::InStock, Source::Manual), at)
        .unwrap();
    db.update_target(&id, false, 0, false).unwrap();
    assert!(db.pending_deliveries().unwrap().is_empty());
    assert!(
        db.record(&input(&id, StockState::OutOfStock, Source::Manual), at)
            .is_err()
    );
    db.update_target(&id, true, 0, false).unwrap();
    let mut s = db.settings().unwrap();
    s.paused = true;
    db.save_settings(&s).unwrap();
    assert!(
        db.record(&input(&id, StockState::OutOfStock, Source::Manual), at)
            .is_err()
    );
    s.stale_minutes = 0;
    assert!(db.save_settings(&s).is_err());
    assert!(db.update_target(&id, true, 3, false).is_err());
}
#[test]
fn export_has_all_samples_beyond_list_limit_and_no_credentials() {
    let (mut db, _, id) = fixture();
    let at = Utc::now().to_rfc3339();
    let tx = db.connection.transaction().unwrap();
    for n in 0..5003 {
        tx.execute(
            "INSERT INTO observations VALUES(?1,?2,'manual','OUT_OF_STOCK',?3,NULL)",
            params![format!("sample-{n}"), id, at],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(db.history(Source::Manual, 7, None).unwrap().len(), 5000);
    let json: serde_json::Value =
        serde_json::from_str(&db.export(Source::Manual, "json").unwrap()).unwrap();
    assert_eq!(json["observations"].as_array().unwrap().len(), 5003);
    let csv = db.export(Source::Manual, "csv").unwrap();
    assert_eq!(csv.lines().count(), 5004);
    assert!(
        !db.export(Source::Manual, "json")
            .unwrap()
            .contains("device_key")
    );
}
#[test]
fn backup_restores_configuration_and_history_without_replaying_pushes() {
    let (mut db, _, id) = fixture();
    alerts(&db);
    db.record(&input(&id, StockState::InStock, Source::Manual), Utc::now())
        .unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    db.backup(file.path()).unwrap();
    db.clear(Source::Manual).unwrap();
    db.restore(file.path()).unwrap();
    assert_eq!(db.history(Source::Manual, 7, None).unwrap().len(), 1);
    assert!(
        db.deliveries()
            .unwrap()
            .iter()
            .all(|d| d.status == "UNCERTAIN")
    );
    assert!(!db.settings().unwrap().bark_enabled);
}
#[test]
fn malformed_backup_is_rejected_before_mutating_current_database() {
    let (mut db, _, id) = fixture();
    db.record(
        &input(&id, StockState::OutOfStock, Source::Manual),
        Utc::now(),
    )
    .unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    db.backup(file.path()).unwrap();
    let backup = Connection::open(file.path()).unwrap();
    backup
        .execute("INSERT INTO settings VALUES(1,'{}')", [])
        .unwrap();
    drop(backup);
    assert!(db.restore(file.path()).is_err());
    assert_eq!(
        db.targets(Source::Manual, Utc::now()).unwrap()[0].state,
        "OUT_OF_STOCK"
    );
    assert_eq!(db.history(Source::Manual, 7, None).unwrap().len(), 1);
}
#[test]
fn future_database_version_is_rejected_without_overwriting_version() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let c = Connection::open(file.path()).unwrap();
    c.execute_batch("PRAGMA user_version=99").unwrap();
    drop(c);
    assert!(Database::open(file.path()).is_err());
    let c = Connection::open(file.path()).unwrap();
    let version: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 99);
}

#[test]
fn restart_preserves_targets_and_history_without_replaying_old_notifications() {
    let (mut db, _, id) = fixture();
    alerts(&db);
    db.record(&input(&id, StockState::InStock, Source::Manual), Utc::now())
        .unwrap();
    let delivery = db.deliveries().unwrap().remove(0);
    db.claim_delivery(&delivery.id).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    db.backup(file.path()).unwrap();
    drop(db);
    let reopened = Database::open(file.path()).unwrap();
    assert_eq!(
        reopened.targets(Source::Manual, Utc::now()).unwrap()[0].id,
        id
    );
    assert_eq!(reopened.history(Source::Manual, 7, None).unwrap().len(), 1);
    let deliveries = reopened.deliveries().unwrap();
    assert_eq!(
        deliveries
            .iter()
            .filter(|d| d.status == "UNCERTAIN")
            .count(),
        1
    );
    assert_eq!(
        deliveries
            .iter()
            .filter(|d| d.status == "CANCELLED")
            .count(),
        1
    );
    assert!(reopened.pending_deliveries().unwrap().is_empty());
}
#[test]
fn official_and_bark_urls_reject_credentials_lookalikes_and_redirect_parameters() {
    for good in [
        "https://www.apple.com.cn/shop/buy-iphone/iphone-17",
        "https://www.apple.com/hk/shop",
    ] {
        assert!(official_url(good).is_ok());
    }
    for bad in [
        "http://apple.com/shop",
        "https://apple.com.evil.test/shop",
        "https://user@apple.com/shop",
        "https://apple.com/shop?redirect=https://evil.test",
        "https://apple.com:8443/shop",
        "https://evil.test/shop",
        "https://apple.com/redirect",
    ] {
        assert!(official_url(bad).is_err(), "{bad}");
    }
    assert!(bark_url("https://api.day.app/TEST_KEY-123").is_ok());
    for bad in [
        "https://api.day.app/",
        "http://api.day.app/key",
        "https://api.day.app/key/title",
        "https://api.day.app/key?x=1",
        "https://api.day.app/%2Fsecret",
        "https://user@api.day.app/key",
    ] {
        assert!(bark_url(bad).is_err(), "{bad}");
    }
}
