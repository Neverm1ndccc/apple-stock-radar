use radar_core::{history::HistoryFilter, *};
use rusqlite::params;

fn fixture() -> (Database, Vec<Target>) {
    let mut db = Database::memory().unwrap();
    let catalog = Catalog::load().unwrap();
    for locale in ["zh_CN", "zh_HK"] {
        let product = catalog
            .products
            .iter()
            .find(|p| p.locale == locale)
            .unwrap();
        let store = catalog.stores.iter().find(|s| s.locale == locale).unwrap();
        db.add_targets(
            &catalog,
            std::slice::from_ref(&product.id),
            std::slice::from_ref(&store.id),
            "历史筛选",
        )
        .unwrap();
    }
    let targets = db.targets(Source::Manual, chrono::Utc::now()).unwrap();
    (db, targets)
}

fn add(
    db: &Database,
    id: &str,
    target: &Target,
    source: &str,
    state: &str,
    at: &str,
    event: Option<&str>,
) {
    db.connection
        .execute(
            "INSERT INTO observations VALUES(?1,?2,?3,?4,?5,NULL)",
            params![id, target.id, source, state, at],
        )
        .unwrap();
    if let Some(event) = event {
        db.connection
            .execute(
                "INSERT INTO events VALUES(?1,?1,?2,?3,?4,?5)",
                params![id, target.id, source, event, at],
            )
            .unwrap();
    }
}

fn filter() -> HistoryFilter {
    HistoryFilter {
        from: Some("2026-10-27T00:00:00Z".into()),
        to: Some("2026-10-29T00:00:00Z".into()),
        ..Default::default()
    }
}

#[test]
fn combined_filters_include_archived_targets_and_keep_source_and_statistics_consistent() {
    let (db, targets) = fixture();
    let cn = targets
        .iter()
        .find(|t| t.product.locale == "zh_CN")
        .unwrap();
    let hk = targets
        .iter()
        .find(|t| t.product.locale == "zh_HK")
        .unwrap();
    add(
        &db,
        "cn-in",
        cn,
        "manual",
        "IN_STOCK",
        "2026-10-28T01:00:00.000Z",
        Some("FIRST_IN_STOCK"),
    );
    add(
        &db,
        "cn-out",
        cn,
        "manual",
        "OUT_OF_STOCK",
        "2026-10-28T02:00:00.000Z",
        None,
    );
    add(
        &db,
        "cn-unknown",
        cn,
        "manual",
        "UNKNOWN",
        "2026-10-28T03:00:00.000Z",
        Some("HEALTH_ALERT"),
    );
    add(
        &db,
        "hk-in",
        hk,
        "manual",
        "IN_STOCK",
        "2026-10-28T01:00:00.000Z",
        Some("FIRST_IN_STOCK"),
    );
    add(
        &db,
        "mock-in",
        cn,
        "mock",
        "IN_STOCK",
        "2026-10-28T01:00:00.000Z",
        None,
    );
    add(
        &db,
        "apple-in",
        cn,
        "apple",
        "IN_STOCK",
        "2026-10-28T01:00:00.000Z",
        None,
    );
    db.connection
        .execute("UPDATE targets SET archived=1 WHERE id=?1", [&cn.id])
        .unwrap();
    let mut f = HistoryFilter {
        target_id: Some(cn.id.clone()),
        state: Some(StockState::InStock),
        locale: Some("zh_CN".into()),
        query: format!("{} {}", cn.product.sku.to_lowercase(), cn.store.city),
        event: Some("FIRST_IN_STOCK".into()),
        ..filter()
    };
    let result = db.query_history(Source::Manual, &f, 480).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].id, "cn-in");
    assert_eq!(
        (
            result.summary.total,
            result.summary.valid,
            result.summary.in_stock,
            result.summary.events
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(result.summary.cells.iter().map(|c| c.total).sum::<u64>(), 1);
    assert_eq!(result.summary.cells[0].hour, 9);
    f.locale = Some("zh_HK".into());
    assert_eq!(
        db.query_history(Source::Manual, &f, 480)
            .unwrap()
            .summary
            .total,
        0
    );
    let all = db.query_history(Source::Manual, &filter(), 480).unwrap();
    assert_eq!(
        (
            all.summary.total,
            all.summary.valid,
            all.summary.in_stock,
            all.summary.events
        ),
        (4, 3, 2, 2)
    );
    let events = db
        .query_history(
            Source::Manual,
            &HistoryFilter {
                event: Some("EVENTS_ONLY".into()),
                ..filter()
            },
            480,
        )
        .unwrap();
    assert_eq!(events.summary.total, 3);
}

#[test]
fn status_filter_precedes_limit_and_summary_counts_all_matches() {
    let (mut db, targets) = fixture();
    let target = &targets[0];
    add(
        &db,
        "old-in",
        target,
        "manual",
        "IN_STOCK",
        "2026-10-27T01:00:00.000Z",
        None,
    );
    let tx = db.connection.transaction().unwrap();
    for n in 0..5005 {
        tx.execute("INSERT INTO observations VALUES(?1,?2,'manual','UNKNOWN','2026-10-28T01:00:00.000Z','TIMEOUT')",params![format!("new-{n}"),target.id]).unwrap();
    }
    tx.commit().unwrap();
    let all = db.query_history(Source::Manual, &filter(), 480).unwrap();
    assert_eq!(all.rows.len(), 5000);
    assert_eq!(all.summary.total, 5006);
    assert_eq!(all.summary.in_stock, 1);
    let found = db
        .query_history(
            Source::Manual,
            &HistoryFilter {
                state: Some(StockState::InStock),
                ..filter()
            },
            480,
        )
        .unwrap();
    assert_eq!(found.rows[0].id, "old-in");
    assert_eq!(found.summary.total, 1);
}

#[test]
fn custom_time_bounds_are_inclusive_and_normalized_before_local_heatmap() {
    let (db, targets) = fixture();
    add(
        &db,
        "midnight",
        &targets[0],
        "manual",
        "IN_STOCK",
        "2026-10-27T16:00:00.000Z",
        None,
    );
    add(
        &db,
        "after",
        &targets[0],
        "manual",
        "OUT_OF_STOCK",
        "2026-10-27T16:00:00.001Z",
        None,
    );
    let f = HistoryFilter {
        from: Some("2026-10-28T00:00:00+08:00".into()),
        to: Some("2026-10-28T00:00:00+08:00".into()),
        ..Default::default()
    };
    let result = db.query_history(Source::Manual, &f, 480).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.summary.first_seen.as_deref(),
        Some("2026-10-27T16:00:00.000Z")
    );
    assert_eq!(result.summary.cells[0].day, "2026-10-28");
    assert_eq!(result.summary.cells[0].hour, 0);
}

#[test]
fn malformed_filters_are_rejected_and_search_metacharacters_are_literal() {
    let (db, targets) = fixture();
    add(
        &db,
        "sample",
        &targets[0],
        "manual",
        "IN_STOCK",
        "2026-10-28T01:00:00.000Z",
        None,
    );
    for bad in [
        HistoryFilter {
            from: Some("invalid".into()),
            ..filter()
        },
        HistoryFilter {
            from: Some("2026-10-30T00:00:00Z".into()),
            ..filter()
        },
        HistoryFilter {
            from: Some("2020-01-01T00:00:00Z".into()),
            ..filter()
        },
        HistoryFilter {
            locale: Some("unknown".into()),
            ..filter()
        },
        HistoryFilter {
            event: Some("unknown".into()),
            ..filter()
        },
        HistoryFilter {
            query: "x".repeat(201),
            ..filter()
        },
    ] {
        assert!(db.query_history(Source::Manual, &bad, 480).is_err());
    }
    for literal in ["' OR 1=1 --", "%", "_"] {
        assert_eq!(
            db.query_history(
                Source::Manual,
                &HistoryFilter {
                    query: literal.into(),
                    ..filter()
                },
                480
            )
            .unwrap()
            .summary
            .total,
            0
        );
    }
    assert!(serde_json::from_value::<HistoryFilter>(serde_json::json!({"state":"STALE"})).is_err());
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}
