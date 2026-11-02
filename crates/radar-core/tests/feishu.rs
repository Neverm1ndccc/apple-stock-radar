use chrono::Utc;
use radar_core::{feishu::*, *};

fn fixture() -> (Database, Vec<Target>) {
    let mut database = Database::memory().unwrap();
    let catalog = Catalog::load().unwrap();
    let product = catalog
        .products
        .iter()
        .find(|p| p.locale == "zh_CN")
        .unwrap();
    let stores: Vec<_> = catalog
        .stores
        .iter()
        .filter(|s| s.locale == "zh_CN")
        .take(2)
        .map(|s| s.id.clone())
        .collect();
    database
        .add_targets(
            &catalog,
            std::slice::from_ref(&product.id),
            &stores,
            "飞书测试",
        )
        .unwrap();
    database
        .save_settings(&Settings {
            feishu_enabled: true,
            ..Settings::default()
        })
        .unwrap();
    let targets = database.targets(Source::Manual, Utc::now()).unwrap();
    (database, targets)
}

fn input(id: &str, state: StockState, source: Source) -> ObservationInput {
    ObservationInput {
        id: uuid::Uuid::new_v4().to_string(),
        target_id: id.into(),
        source,
        state,
        reason: (state == StockState::Unknown).then(|| "TIMEOUT".into()),
    }
}

#[test]
fn selected_targets_only_notify_on_stock_changes_and_never_replay_on_enable() {
    let (mut db, targets) = fixture();
    let id = &targets[0].id;
    db.set_target_feishu(id, true).unwrap();
    let stock = input(id, StockState::InStock, Source::Manual);
    db.record(&stock, Utc::now()).unwrap();
    db.record(&stock, Utc::now()).unwrap();
    db.record(&input(id, StockState::InStock, Source::Manual), Utc::now())
        .unwrap();
    db.record(
        &input(&targets[1].id, StockState::InStock, Source::Manual),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(db.deliveries().unwrap().len(), 1);
    assert_eq!(db.deliveries().unwrap()[0].channel, "feishu");
    db.record(&input(id, StockState::Unknown, Source::Manual), Utc::now())
        .unwrap();
    assert_eq!(db.deliveries().unwrap().len(), 1);
    db.record(&input(id, StockState::InStock, Source::Manual), Utc::now())
        .unwrap();
    assert_eq!(db.deliveries().unwrap().len(), 2);
    db.set_target_feishu(id, false).unwrap();
    assert!(db.pending_deliveries().unwrap().is_empty());
    db.set_target_feishu(id, true).unwrap();
    assert!(db.pending_deliveries().unwrap().is_empty());
    assert_eq!(db.history(Source::Manual, 7, None).unwrap().len(), 5);
}

#[test]
fn global_switch_target_pause_and_source_are_rechecked_before_claim() {
    let (mut db, targets) = fixture();
    let id = &targets[0].id;
    db.set_target_feishu(id, true).unwrap();
    db.record(&input(id, StockState::InStock, Source::Manual), Utc::now())
        .unwrap();
    let delivery = db.deliveries().unwrap().remove(0);
    let mut settings = db.settings().unwrap();
    settings.feishu_enabled = false;
    db.save_settings(&settings).unwrap();
    assert!(
        !db.claim_feishu_delivery(&delivery.id, id, Source::Manual)
            .unwrap()
    );
    settings.feishu_enabled = true;
    db.save_settings(&settings).unwrap();
    db.update_target(id, false, 0, false).unwrap();
    // Preference remains checked while the target is paused; no push is eligible.
    assert!(
        db.targets(Source::Manual, Utc::now())
            .unwrap()
            .iter()
            .find(|t| &t.id == id)
            .unwrap()
            .feishu_enabled
    );
    assert!(
        !db.claim_feishu_delivery(&delivery.id, id, Source::Manual)
            .unwrap()
    );
    db.update_target(id, true, 0, false).unwrap();
    db.record(
        &input(id, StockState::OutOfStock, Source::Manual),
        Utc::now(),
    )
    .unwrap();
    db.record(&input(id, StockState::InStock, Source::Manual), Utc::now())
        .unwrap();
    let next = db.pending_deliveries().unwrap().remove(0);
    assert!(
        db.claim_feishu_delivery(&next.id, id, Source::Manual)
            .unwrap()
    );
    assert!(
        !db.claim_feishu_delivery(&next.id, id, Source::Manual)
            .unwrap()
    );
    settings.mode = Source::Mock;
    db.save_settings(&settings).unwrap();
    for _ in 0..6 {
        db.mock_tick().unwrap();
    }
    assert!(db.pending_deliveries().unwrap().is_empty());
    assert!(target_message(&db.targets(Source::Mock, Utc::now()).unwrap()[0]).is_err());
}

#[test]
fn global_disable_cancels_pending_and_user_settings_cannot_bypass_keychain_enable() {
    let (mut db, targets) = fixture();
    db.set_target_feishu(&targets[0].id, true).unwrap();
    db.record(
        &input(&targets[0].id, StockState::InStock, Source::Manual),
        Utc::now(),
    )
    .unwrap();
    db.cancel_pending_feishu().unwrap();
    assert!(db.pending_deliveries().unwrap().is_empty());
    let mut previous = Settings::default();
    let forged = Settings {
        feishu_enabled: true,
        ..previous.clone()
    };
    assert!(!forged.user_update(&previous, false).unwrap().feishu_enabled);
    previous.feishu_enabled = true;
    assert!(
        Settings::default()
            .user_update(&previous, false)
            .unwrap()
            .feishu_enabled
    );
}

#[test]
fn v2_upgrade_and_old_backup_preserve_data_default_off_and_v3_restore_does_not_push() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v2.sqlite");
    let (mut db, targets) = fixture();
    let id = &targets[0].id;
    db.record(&input(id, StockState::InStock, Source::Manual), Utc::now())
        .unwrap();
    db.connection.execute_batch("DROP TABLE target_notifications; UPDATE schema_version SET version=2; PRAGMA user_version=2;").unwrap();
    let mut legacy_settings = serde_json::to_value(db.settings().unwrap()).unwrap();
    legacy_settings
        .as_object_mut()
        .unwrap()
        .remove("feishu_enabled");
    db.connection
        .execute("UPDATE settings SET json=?1", [legacy_settings.to_string()])
        .unwrap();
    let old_backup = directory.path().join("old-v2.sqlite");
    db.backup(&old_backup).unwrap();
    db.backup(&path).unwrap();
    let mut upgraded = Database::open(&path).unwrap();
    assert_eq!(
        upgraded
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        upgraded.targets(Source::Manual, Utc::now()).unwrap().len(),
        2
    );
    assert_eq!(upgraded.history(Source::Manual, 7, None).unwrap().len(), 1);
    assert!(!upgraded.targets(Source::Manual, Utc::now()).unwrap()[0].feishu_enabled);
    assert!(!upgraded.settings().unwrap().feishu_enabled);
    upgraded.set_target_feishu(id, true).unwrap();
    let mut enabled = upgraded.settings().unwrap();
    enabled.feishu_enabled = true;
    upgraded.save_settings(&enabled).unwrap();
    let backup = directory.path().join("v3.sqlite");
    upgraded.backup(&backup).unwrap();
    upgraded.restore(&backup).unwrap();
    assert!(!upgraded.settings().unwrap().feishu_enabled);
    assert!(
        upgraded
            .targets(Source::Manual, Utc::now())
            .unwrap()
            .iter()
            .find(|t| &t.id == id)
            .unwrap()
            .feishu_enabled
    );
    assert!(upgraded.pending_deliveries().unwrap().is_empty());
    upgraded.restore(&old_backup).unwrap();
    assert!(!upgraded.settings().unwrap().feishu_enabled);
    assert!(!upgraded.targets(Source::Manual, Utc::now()).unwrap()[0].feishu_enabled);
    assert_eq!(upgraded.history(Source::Manual, 7, None).unwrap().len(), 1);
}

#[test]
fn official_webhooks_signatures_and_responses_do_not_accept_lookalikes_or_leak_secrets() {
    let webhook = "https://open.feishu.cn/open-apis/bot/v2/hook/test-token";
    webhook_url(webhook).unwrap();
    for invalid in [
        "http://open.feishu.cn/open-apis/bot/v2/hook/key",
        "https://open.feishu.cn.evil.test/open-apis/bot/v2/hook/key",
        "https://user:secret@open.feishu.cn/open-apis/bot/v2/hook/key",
        "https://open.feishu.cn/open-apis/bot/v2/hook/key?redirect=evil",
        "https://open.feishu.cn/open-apis/bot/v2/hook/key#secret",
        "https://open.feishu.cn/open-apis/bot/v2/hook/a/b",
        "https://open.feishu.cn/open-apis/bot/v2/hook/",
    ] {
        assert!(webhook_url(invalid).is_err());
    }
    let value = payload(
        "产品",
        "门店",
        "https://www.apple.com.cn/shop",
        Some("test-secret"),
        1700000000,
    )
    .unwrap();
    assert_eq!(
        value["sign"],
        "mbm4Y4oluIPQ00qlBIhX8vAZ0EKv3nw0LuTb91jPL84="
    );
    assert_eq!(value["timestamp"], "1700000000");
    assert!(
        value["content"]["text"]
            .as_str()
            .unwrap()
            .contains("库存雷达")
    );
    assert!(!value.to_string().contains("test-secret"));
    assert!(
        payload(
            "标题",
            &"a".repeat(21 * 1024),
            "https://www.apple.com.cn/shop",
            None,
            0
        )
        .is_err()
    );
    check_response(&serde_json::json!({"code":0})).unwrap();
    check_response(&serde_json::json!({"StatusCode":0})).unwrap();
    for response in [
        serde_json::json!({}),
        serde_json::json!({"code":"0"}),
        serde_json::json!({"code":0,"StatusCode":1}),
        serde_json::json!({"code":1,"msg":webhook}),
    ] {
        let error = check_response(&response).unwrap_err();
        assert!(!error.contains("test-token"));
    }
}

#[test]
fn manual_status_messages_preserve_unknown_expiry_and_do_not_change_history() {
    let (db, targets) = fixture();
    let mut target = targets[0].clone();
    for state in ["UNKNOWN", "PENDING", "UNSUPPORTED", "STALE"] {
        target.state = state.into();
        assert!(!target_message(&target).unwrap().0.contains("可取货"));
    }
    assert!(db.history(Source::Manual, 7, None).unwrap().is_empty());
}
