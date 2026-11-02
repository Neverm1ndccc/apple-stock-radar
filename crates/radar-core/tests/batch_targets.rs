use chrono::Utc;
use radar_core::*;

fn fixture() -> (Database, Vec<String>) {
    let mut db = Database::memory().unwrap();
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
        .take(3)
        .map(|s| s.id.clone())
        .collect();
    db.add_targets(
        &catalog,
        std::slice::from_ref(&product.id),
        &stores,
        "批量回归",
    )
    .unwrap();
    let mut ids: Vec<_> = db
        .targets(Source::Manual, Utc::now())
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    ids.sort();
    (db, ids)
}

#[test]
fn batch_changes_only_selected_fields_and_keeps_unselected_targets_and_settings() {
    let (mut db, ids) = fixture();
    let settings = serde_json::to_value(db.settings().unwrap()).unwrap();
    let selected = vec![ids[0].clone(), ids[1].clone(), ids[0].clone()];
    assert_eq!(
        db.batch_update_targets(&selected, TargetBatchAction::EnableFeishu)
            .unwrap(),
        2
    );
    db.batch_update_targets(&selected, TargetBatchAction::HighPriority)
        .unwrap();
    db.batch_update_targets(&selected, TargetBatchAction::Pause)
        .unwrap();
    let targets = db.targets(Source::Manual, Utc::now()).unwrap();
    for target in &targets {
        let chosen = selected.contains(&target.id);
        assert_eq!(target.feishu_enabled, chosen);
        assert_eq!(target.enabled, !chosen);
        assert_eq!(target.priority, if chosen { 2 } else { 0 });
    }
    db.batch_update_targets(&selected, TargetBatchAction::Resume)
        .unwrap();
    db.batch_update_targets(&selected, TargetBatchAction::NormalPriority)
        .unwrap();
    assert!(
        db.targets(Source::Manual, Utc::now())
            .unwrap()
            .iter()
            .all(|t| t.enabled && t.priority == 0)
    );
    assert_eq!(
        serde_json::to_value(db.settings().unwrap()).unwrap(),
        settings
    );
    assert!(db.deliveries().unwrap().is_empty());
    assert!(db.history(Source::Manual, 7, None).unwrap().is_empty());
}

#[test]
fn missing_archived_or_invalid_selection_changes_nothing() {
    let (mut db, ids) = fixture();
    let invalid = vec![ids[0].clone(), "zz-missing".into()];
    assert!(
        db.batch_update_targets(&invalid, TargetBatchAction::EnableFeishu)
            .is_err()
    );
    assert!(
        db.targets(Source::Manual, Utc::now())
            .unwrap()
            .iter()
            .all(|t| !t.feishu_enabled)
    );
    db.update_target(&ids[1], true, 0, true).unwrap();
    assert!(
        db.batch_update_targets(&ids[..2], TargetBatchAction::Pause)
            .is_err()
    );
    assert!(
        db.targets(Source::Manual, Utc::now())
            .unwrap()
            .iter()
            .all(|t| t.enabled)
    );
    assert!(
        db.batch_update_targets(&[], TargetBatchAction::Pause)
            .is_err()
    );
    assert!(
        db.batch_update_targets(&vec![ids[0].clone(); 5001], TargetBatchAction::Pause)
            .is_err()
    );
    assert!(serde_json::from_str::<TargetBatchAction>("\"delete_everything\"").is_err());
}

#[test]
fn sqlite_failure_rolls_back_preceding_target_updates() {
    let (mut db, ids) = fixture();
    db.connection.execute_batch(&format!("CREATE TRIGGER fail_second BEFORE UPDATE ON targets WHEN NEW.id='{}' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;", ids[1])).unwrap();
    assert!(
        db.batch_update_targets(&ids[..2], TargetBatchAction::Pause)
            .is_err()
    );
    assert!(
        db.targets(Source::Manual, Utc::now())
            .unwrap()
            .iter()
            .all(|t| t.enabled)
    );
}

#[test]
fn batch_disable_and_pause_cancel_only_matching_pending_messages_without_replaying_history() {
    let (mut db, ids) = fixture();
    db.save_settings(&Settings {
        feishu_enabled: true,
        local_notifications: true,
        bark_enabled: true,
        ..Settings::default()
    })
    .unwrap();
    db.batch_update_targets(&ids, TargetBatchAction::EnableFeishu)
        .unwrap();
    for id in &ids {
        db.record(
            &ObservationInput {
                id: uuid::Uuid::new_v4().to_string(),
                target_id: id.clone(),
                state: StockState::InStock,
                source: Source::Manual,
                reason: None,
            },
            Utc::now(),
        )
        .unwrap();
    }
    assert_eq!(db.pending_deliveries().unwrap().len(), 9);
    db.batch_update_targets(&ids[..1], TargetBatchAction::DisableFeishu)
        .unwrap();
    let pending = db.pending_deliveries().unwrap();
    assert_eq!(pending.len(), 8);
    assert_eq!(pending.iter().filter(|d| d.target_id == ids[0]).count(), 2);
    assert!(
        !pending
            .iter()
            .any(|d| d.target_id == ids[0] && d.channel == "feishu")
    );
    db.batch_update_targets(&ids[1..2], TargetBatchAction::Pause)
        .unwrap();
    assert_eq!(db.pending_deliveries().unwrap().len(), 5);
    assert!(
        !db.pending_deliveries()
            .unwrap()
            .iter()
            .any(|d| d.target_id == ids[1])
    );
    db.batch_update_targets(&ids[..1], TargetBatchAction::EnableFeishu)
        .unwrap();
    db.batch_update_targets(&ids[1..2], TargetBatchAction::Resume)
        .unwrap();
    assert_eq!(db.pending_deliveries().unwrap().len(), 5);
    assert_eq!(db.history(Source::Manual, 7, None).unwrap().len(), 3);
}
