//! Offline fixture for the browser-only IPC harness; never reads the user's database.
use chrono::Utc;
use radar_core::*;

fn main() {
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
        .filter(|s| s.locale == "zh_CN" && s.city == "上海")
        .take(3)
        .map(|s| s.id.clone())
        .collect();
    db.add_targets(&catalog, &products, &stores, "界面验收样本")
        .unwrap();
    db.save_settings(&Settings {
        mode: Source::Mock,
        ..Settings::default()
    })
    .unwrap();
    for _ in 0..5 {
        db.mock_tick().unwrap();
    }
    let snapshot = Snapshot {
        targets: db.targets(Source::Mock, Utc::now()).unwrap(),
        settings: db.settings().unwrap(),
        deliveries: db.deliveries().unwrap(),
        groups: db.groups().unwrap(),
        catalog_version: catalog.version.clone(),
        automatic_available: false,
        scanner: radar_core::scanner::ScanStatus {
            budget_capacity: radar_core::request_budget::CAPACITY,
            budget_remaining: radar_core::request_budget::CAPACITY,
            budget_refill_seconds: radar_core::request_budget::REFILL_SECONDS,
            ..radar_core::scanner::ScanStatus::default()
        },
    };
    println!(
        "{}",
        serde_json::json!({"catalog":catalog,"snapshot":snapshot,"history":db.history(Source::Mock,7,None).unwrap(),"summary":db.summary(Source::Mock,7,None,480).unwrap()})
    );
}
