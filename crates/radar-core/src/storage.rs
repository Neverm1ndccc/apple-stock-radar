use crate::*;
use chrono::{DateTime, Duration, FixedOffset, Timelike, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::BTreeMap, path::Path, time::Duration as StdDuration};
use uuid::Uuid;

pub struct Database {
    pub connection: Connection,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).map_err(err)?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if matches!(version, 1 | 2) {
            connection
                .backup(
                    "main",
                    path.with_extension(format!("pre-v3-{}.sqlite", Uuid::new_v4())),
                    None,
                )
                .map_err(err)?;
        }
        Self::init(connection)
    }
    pub fn memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(err)?)
    }

    fn init(connection: Connection) -> Result<Self> {
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if ![0, 1, 2, 3].contains(&version) {
            return Err("数据库版本不兼容，原数据未修改".into());
        }
        connection
            .busy_timeout(StdDuration::from_secs(5))
            .map_err(err)?;
        if version == 1 {
            Self::migrate_v1(&connection)?;
        }
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
            INSERT INTO schema_version SELECT 2 WHERE NOT EXISTS(SELECT 1 FROM schema_version);
            CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS variants(id TEXT PRIMARY KEY, locale TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS stores(id TEXT PRIMARY KEY, locale TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS watch_groups(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS targets(
                id TEXT PRIMARY KEY, variant_id TEXT NOT NULL REFERENCES variants(id),
                store_key TEXT NOT NULL REFERENCES stores(id), enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL, UNIQUE(variant_id,store_key));
            CREATE TABLE IF NOT EXISTS group_members(
                group_id TEXT REFERENCES watch_groups(id) ON DELETE CASCADE,
                target_id TEXT REFERENCES targets(id), PRIMARY KEY(group_id,target_id));
            CREATE TABLE IF NOT EXISTS observations(
                id TEXT PRIMARY KEY,target_id TEXT NOT NULL REFERENCES targets(id),
                source TEXT NOT NULL CHECK(source IN ('manual','mock','apple')),
                state TEXT NOT NULL CHECK(state IN ('IN_STOCK','OUT_OF_STOCK','UNKNOWN')),
                observed_at TEXT NOT NULL,reason TEXT);
            CREATE INDEX IF NOT EXISTS observations_target_time ON observations(target_id,source,observed_at DESC);
            CREATE TABLE IF NOT EXISTS events(
                id TEXT PRIMARY KEY,observation_id TEXT NOT NULL UNIQUE REFERENCES observations(id) ON DELETE CASCADE,
                target_id TEXT NOT NULL REFERENCES targets(id),source TEXT NOT NULL,
                kind TEXT NOT NULL,created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS deliveries(
                id TEXT PRIMARY KEY,event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
                channel TEXT NOT NULL,status TEXT NOT NULL,attempted_at TEXT,error TEXT,
                UNIQUE(event_id,channel));
            PRAGMA user_version=2;") .map_err(err)?;
        Self::migrate_feishu(&connection)?;
        let db = Self { connection };
        // Sending was interrupted by process exit. Do not silently resend an uncertain push.
        db.connection.execute("UPDATE deliveries SET status='UNCERTAIN',error='应用退出时投递尚未确认，未自动重发' WHERE status='SENDING'", []).map_err(err)?;
        db.connection.execute("UPDATE deliveries SET status='CANCELLED',error='应用重启后未补发旧提醒' WHERE status='PENDING'", []).map_err(err)?;
        let mut settings = db.settings()?;
        if settings.quiesce_scan_alert() {
            db.save_settings(&settings)?;
        }
        Ok(db)
    }

    fn migrate_v1(connection: &Connection) -> Result<()> {
        connection.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;
            CREATE TABLE observations_v2(
                id TEXT PRIMARY KEY,target_id TEXT NOT NULL REFERENCES targets(id),
                source TEXT NOT NULL CHECK(source IN ('manual','mock','apple')),
                state TEXT NOT NULL CHECK(state IN ('IN_STOCK','OUT_OF_STOCK','UNKNOWN')),
                observed_at TEXT NOT NULL,reason TEXT);
            INSERT INTO observations_v2 SELECT * FROM observations;
            DROP TABLE observations;
            ALTER TABLE observations_v2 RENAME TO observations;
            CREATE INDEX observations_target_time ON observations(target_id,source,observed_at DESC);
            UPDATE schema_version SET version=2; PRAGMA user_version=2; COMMIT; PRAGMA foreign_keys=ON;").map_err(err)
    }

    fn migrate_feishu(connection: &Connection) -> Result<()> {
        connection
            .execute_batch(
                "BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS target_notifications(
                target_id TEXT PRIMARY KEY REFERENCES targets(id),
                feishu_enabled INTEGER NOT NULL DEFAULT 0 CHECK(feishu_enabled IN (0,1)));
            UPDATE schema_version SET version=3; PRAGMA user_version=3; COMMIT;",
            )
            .map_err(err)
    }

    pub fn settings(&self) -> Result<Settings> {
        let json: Option<String> = self
            .connection
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        let settings: Settings = match json {
            Some(s) => {
                serde_json::from_str(&s).map_err(|e| format!("设置损坏，保留原数据：{e}"))?
            }
            None => Settings::default(),
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        self.connection.execute("INSERT INTO settings(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json", [serde_json::to_string(settings).map_err(err)?]).map_err(err)?;
        Ok(())
    }

    /// Re-read and reserve in one SQLite write transaction so restart/other windows
    /// cannot obtain a fresh budget between reading settings and sending a request.
    pub fn reserve_scan_request(
        &mut self,
        at: DateTime<Utc>,
        minimum: u64,
        force: bool,
    ) -> Result<bool> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let json: Option<String> = tx
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        let mut settings: Settings = match json {
            Some(json) => serde_json::from_str(&json).map_err(err)?,
            None => Settings::default(),
        };
        settings.validate()?;
        if settings.mode != Source::Apple
            || settings.paused
            || (!force && !settings.scan_running)
            || settings.scan_stop_reason.is_some()
        {
            return Ok(false);
        }
        let due = settings
            .scan_last_request_at
            .map(|last| last + Duration::seconds(settings.effective_scan_interval(minimum) as i64))
            .into_iter()
            .chain(settings.scan_retry_after)
            .chain(settings.budget_ready_at())
            .max();
        if due.is_some_and(|due| due > at) {
            return Ok(false);
        }
        let mut budget = settings.request_budget(at);
        if !budget.take(at) {
            return Ok(false);
        }
        settings.scan_budget = Some(budget);
        settings.scan_last_request_at = Some(at);
        settings.validate()?;
        tx.execute("INSERT INTO settings(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json",[serde_json::to_string(&settings).map_err(err)?]).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(true)
    }

    pub fn add_targets(
        &mut self,
        catalog: &Catalog,
        product_ids: &[String],
        store_ids: &[String],
        title: &str,
    ) -> Result<AddResult> {
        if product_ids.is_empty() || store_ids.is_empty() {
            return Err("至少选择一个 SKU 和一家门店".into());
        }
        if product_ids.len().saturating_mul(store_ids.len()) > 500 {
            return Err("单次最多创建 500 个组合".into());
        }
        let products = product_ids
            .iter()
            .map(|id| catalog.product(id))
            .collect::<Result<Vec<_>>>()?;
        let locale = &products[0].locale;
        if products.iter().any(|p| &p.locale != locale) {
            return Err("一次只能添加同一地区的商品和门店".into());
        }
        let stores = store_ids
            .iter()
            .map(|id| catalog.store(locale, id))
            .collect::<Result<Vec<_>>>()?;
        let tx = self.connection.transaction().map_err(err)?;
        let group = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO watch_groups VALUES(?1,?2,?3)",
            params![
                group,
                title.trim().chars().take(80).collect::<String>(),
                now()
            ],
        )
        .map_err(err)?;
        let mut added = 0;
        let mut duplicates = 0;
        for p in &products {
            tx.execute(
                "INSERT OR IGNORE INTO variants VALUES(?1,?2,?3)",
                params![p.id, p.locale, serde_json::to_string(p).map_err(err)?],
            )
            .map_err(err)?;
            for s in &stores {
                let key = format!("{}:{}", s.locale, s.id);
                tx.execute(
                    "INSERT OR IGNORE INTO stores VALUES(?1,?2,?3)",
                    params![key, s.locale, serde_json::to_string(s).map_err(err)?],
                )
                .map_err(err)?;
                let existing: Option<(String, bool)> = tx
                    .query_row(
                        "SELECT id,archived FROM targets WHERE variant_id=?1 AND store_key=?2",
                        params![p.id, key],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(err)?;
                let id = if let Some((id, archived)) = existing {
                    if archived {
                        tx.execute("UPDATE targets SET archived=0,enabled=1 WHERE id=?1", [&id])
                            .map_err(err)?;
                        added += 1;
                    } else {
                        duplicates += 1;
                    }
                    id
                } else {
                    let id = Uuid::new_v4().to_string();
                    tx.execute("INSERT INTO targets(id,variant_id,store_key,created_at) VALUES(?1,?2,?3,?4)", params![id,p.id,key,now()]).map_err(err)?;
                    added += 1;
                    id
                };
                tx.execute(
                    "INSERT OR IGNORE INTO group_members VALUES(?1,?2)",
                    params![group, id],
                )
                .map_err(err)?;
            }
        }
        tx.commit().map_err(err)?;
        Ok(AddResult {
            added,
            duplicates,
            combinations: product_ids.len() * store_ids.len(),
        })
    }

    pub fn groups(&self) -> Result<Vec<WatchGroup>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id,title FROM watch_groups ORDER BY created_at DESC,id")
            .map_err(err)?;
        let groups = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut result = Vec::new();
        for (id, title) in groups {
            let mut stmt = self.connection.prepare("SELECT m.target_id FROM group_members m JOIN targets t ON t.id=m.target_id WHERE m.group_id=?1 AND t.archived=0 ORDER BY m.target_id").map_err(err)?;
            let target_ids = stmt
                .query_map([&id], |r| r.get(0))
                .map_err(err)?
                .collect::<std::result::Result<Vec<String>, _>>()
                .map_err(err)?;
            if !target_ids.is_empty() {
                result.push(WatchGroup {
                    id,
                    title,
                    target_ids,
                });
            }
        }
        Ok(result)
    }

    pub fn update_target(
        &self,
        id: &str,
        enabled: bool,
        priority: i64,
        archived: bool,
    ) -> Result<()> {
        if !(0..=2).contains(&priority) {
            return Err("优先级无效".into());
        }
        if self
            .connection
            .execute(
                "UPDATE targets SET enabled=?2,priority=?3,archived=?4 WHERE id=?1",
                params![id, enabled, priority, archived],
            )
            .map_err(err)?
            == 0
        {
            return Err("关注不存在".into());
        }
        if !enabled || archived {
            self.connection.execute("UPDATE deliveries SET status='CANCELLED',error='目标已暂停或移除' WHERE status='PENDING' AND event_id IN (SELECT id FROM events WHERE target_id=?1)",[id]).map_err(err)?;
        }
        Ok(())
    }

    pub fn targets(&self, source: Source, at: DateTime<Utc>) -> Result<Vec<Target>> {
        let settings = self.settings()?;
        let mut stmt = self.connection.prepare("SELECT t.id,v.json,s.json,t.enabled,t.priority FROM targets t JOIN variants v ON v.id=t.variant_id JOIN stores s ON s.id=t.store_key WHERE t.archived=0 ORDER BY t.priority DESC,t.created_at,t.id").map_err(err)?;
        let base = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut result = Vec::new();
        for (id, product, store, enabled, priority) in base {
            let latest: Option<(String,String,Option<String>)> = self.connection.query_row("SELECT state,observed_at,reason FROM observations WHERE target_id=?1 AND source=?2 ORDER BY observed_at DESC,rowid DESC LIMIT 1",params![id,source.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(err)?;
            let valid: Option<(String,String)> = self.connection.query_row("SELECT state,observed_at FROM observations WHERE target_id=?1 AND source=?2 AND state!='UNKNOWN' ORDER BY observed_at DESC,rowid DESC LIMIT 1",params![id,source.as_str()],|r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
            let (state, observed_at, reason) = match latest {
                None => ("PENDING".into(), None, None),
                Some((state, time, reason)) => {
                    let stale = DateTime::parse_from_rfc3339(&time)
                        .map(|t| {
                            at.signed_duration_since(t) > Duration::minutes(settings.stale_minutes)
                        })
                        .unwrap_or(true);
                    let display = if reason.as_deref() == Some("UNSUPPORTED") {
                        "UNSUPPORTED".into()
                    } else if state == "UNKNOWN" {
                        "UNKNOWN".into()
                    } else if stale {
                        "STALE".into()
                    } else {
                        state
                    };
                    (display, Some(time), reason)
                }
            };
            result.push(Target {
                feishu_enabled: self.connection.query_row("SELECT COALESCE((SELECT feishu_enabled FROM target_notifications WHERE target_id=?1),0)", [&id], |r| r.get(0)).map_err(err)?,
                id,
                product: serde_json::from_str(&product).map_err(err)?,
                store: serde_json::from_str(&store).map_err(err)?,
                enabled,
                priority,
                state,
                last_state: valid.as_ref().map(|v| v.0.clone()),
                last_valid_at: valid.map(|v| v.1),
                observed_at,
                reason,
                source,
            });
        }
        Ok(result)
    }

    pub fn feishu_target_enabled(&self, id: &str) -> Result<bool> {
        self.connection.query_row("SELECT EXISTS(SELECT 1 FROM target_notifications n JOIN targets t ON t.id=n.target_id WHERE t.id=?1 AND n.feishu_enabled=1 AND t.enabled=1 AND t.archived=0)", [id], |r| r.get(0)).map_err(err)
    }

    pub fn set_target_feishu(&self, id: &str, enabled: bool) -> Result<()> {
        let exists: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM targets WHERE id=?1 AND archived=0)",
                [id],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            return Err("关注不存在".into());
        }
        self.connection.execute("INSERT INTO target_notifications(target_id,feishu_enabled) VALUES(?1,?2) ON CONFLICT(target_id) DO UPDATE SET feishu_enabled=excluded.feishu_enabled", params![id,enabled]).map_err(err)?;
        if !enabled {
            self.connection.execute("UPDATE deliveries SET status='CANCELLED',error='该关注的飞书通知已关闭' WHERE channel='feishu' AND status='PENDING' AND event_id IN (SELECT id FROM events WHERE target_id=?1)", [id]).map_err(err)?;
        }
        Ok(())
    }

    pub fn cancel_pending_feishu(&self) -> Result<()> {
        self.connection.execute("UPDATE deliveries SET status='CANCELLED',error='飞书通知已关闭或配置已更换' WHERE channel='feishu' AND status='PENDING'", []).map_err(err)?;
        let mut settings = self.settings()?;
        settings.cancel_scan_alert();
        self.save_settings(&settings)
    }

    pub fn claim_scan_alert(&self) -> Result<Option<crate::scan_health::ScanAlert>> {
        let mut settings = self.settings()?;
        let Some(alert) = settings.claim_scan_alert() else {
            return Ok(None);
        };
        let changed = self.connection.execute("UPDATE settings SET json=json_set(json,'$.scan_alert.status','SENDING') WHERE id=1 AND json_extract(json,'$.scan_alert.id')=?1 AND json_extract(json,'$.scan_alert.status')='PENDING' AND json_extract(json,'$.feishu_enabled')=1 AND json_extract(json,'$.scan_failure_notifications')=1 AND json_extract(json,'$.paused')=0 AND json_extract(json,'$.mode')='apple'", [&alert.id]).map_err(err)?;
        Ok((changed == 1).then_some(alert))
    }

    pub fn finish_scan_alert(&self, id: &str, result: Result<()>) -> Result<()> {
        let mut settings = self.settings()?;
        settings.finish_scan_alert(id, result);
        if let Some(alert) = settings.scan_alert.filter(|a| a.id == id) {
            let status = serde_json::to_value(alert.status).map_err(err)?;
            self.connection.execute("UPDATE settings SET json=json_set(json,'$.scan_alert.status',?2,'$.scan_alert.error',?3) WHERE id=1 AND json_extract(json,'$.scan_alert.id')=?1 AND json_extract(json,'$.scan_alert.status')='SENDING'",params![id,status.as_str(),alert.error]).map_err(err)?;
        }
        Ok(())
    }

    pub fn batch_update_targets(
        &mut self,
        ids: &[String],
        action: TargetBatchAction,
    ) -> Result<usize> {
        if ids.is_empty() || ids.len() > 5000 {
            return Err("请选择 1–5000 个关注目标".into());
        }
        let ids: std::collections::BTreeSet<_> = ids.iter().map(String::as_str).collect();
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        // Check the whole selection before changing anything. Archived/stale IDs
        // must fail the batch, never silently turn it into a partial update.
        for id in &ids {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM targets WHERE id=?1 AND archived=0)",
                    [id],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if !exists {
                return Err("所选关注已发生变化，请刷新后重新选择".into());
            }
        }
        for id in &ids {
            match action {
                TargetBatchAction::EnableFeishu | TargetBatchAction::DisableFeishu => {
                    let enabled = matches!(action, TargetBatchAction::EnableFeishu);
                    tx.execute("INSERT INTO target_notifications(target_id,feishu_enabled) VALUES(?1,?2) ON CONFLICT(target_id) DO UPDATE SET feishu_enabled=excluded.feishu_enabled", params![id,enabled]).map_err(err)?;
                    if !enabled {
                        tx.execute("UPDATE deliveries SET status='CANCELLED',error='该关注的飞书通知已关闭' WHERE channel='feishu' AND status='PENDING' AND event_id IN (SELECT id FROM events WHERE target_id=?1)", [id]).map_err(err)?;
                    }
                }
                TargetBatchAction::Pause | TargetBatchAction::Resume => {
                    let enabled = matches!(action, TargetBatchAction::Resume);
                    tx.execute(
                        "UPDATE targets SET enabled=?2 WHERE id=?1",
                        params![id, enabled],
                    )
                    .map_err(err)?;
                    if !enabled {
                        tx.execute("UPDATE deliveries SET status='CANCELLED',error='目标已暂停或移除' WHERE status='PENDING' AND event_id IN (SELECT id FROM events WHERE target_id=?1)", [id]).map_err(err)?;
                    }
                }
                TargetBatchAction::HighPriority | TargetBatchAction::NormalPriority => {
                    let priority = if matches!(action, TargetBatchAction::HighPriority) {
                        2
                    } else {
                        0
                    };
                    tx.execute(
                        "UPDATE targets SET priority=?2 WHERE id=?1",
                        params![id, priority],
                    )
                    .map_err(err)?;
                }
            }
        }
        tx.commit().map_err(err)?;
        Ok(ids.len())
    }

    pub fn record(
        &mut self,
        input: &ObservationInput,
        at: DateTime<Utc>,
    ) -> Result<Option<String>> {
        if input.id.len() > 100 || input.id.is_empty() {
            return Err("观测标识无效".into());
        }
        let settings = self.settings()?;
        if settings.paused {
            return Err("应用已暂停，请恢复后记录".into());
        }
        if input.source != settings.mode {
            return Err("观测来源与当前模式不符，请刷新后重试".into());
        }
        const REASONS: &[&str] = &[
            "SERVICE_UNAVAILABLE",
            "BLOCKED",
            "RATE_LIMITED",
            "NETWORK_ERROR",
            "TIMEOUT",
            "PARSE_ERROR",
            "UNAUTHORIZED",
            "UNSUPPORTED",
            "NO_MATCH",
            "USER_UNCERTAIN",
        ];
        if input.state == StockState::Unknown
            && !input
                .reason
                .as_deref()
                .is_some_and(|r| REASONS.contains(&r))
        {
            return Err("未知状态必须选择有效原因".into());
        }
        if input.state != StockState::Unknown && input.reason.is_some() {
            return Err("有效观测不应携带失败原因".into());
        }
        let tx = self.connection.transaction().map_err(err)?;
        let previous_id: Option<(String, String, String, Option<String>)> = tx
            .query_row(
                "SELECT target_id,source,state,reason FROM observations WHERE id=?1",
                [&input.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(err)?;
        if let Some((id, source, state, reason)) = previous_id {
            if id != input.target_id
                || source != input.source.as_str()
                || state != input.state.as_str()
                || reason != input.reason
            {
                return Err("观测标识已被不同内容使用".into());
            }
            return Ok(None);
        }
        let enabled: Option<bool> = tx
            .query_row(
                "SELECT enabled FROM targets WHERE id=?1 AND archived=0",
                [&input.target_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        if enabled != Some(true) {
            return Err("关注不存在或已暂停".into());
        }
        let previous: Option<(String,String)> = tx.query_row("SELECT state,observed_at FROM observations WHERE target_id=?1 AND source=?2 ORDER BY observed_at DESC,rowid DESC LIMIT 1", params![input.target_id,input.source.as_str()], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
        // An elapsed trustworthy sample cannot establish a continuous restock interval.
        let prev = previous.as_ref().map(|(s, t)| {
            let expired = DateTime::parse_from_rfc3339(t)
                .map(|t| at.signed_duration_since(t) > Duration::minutes(settings.stale_minutes))
                .unwrap_or(true);
            if expired {
                StockState::Unknown
            } else {
                StockState::parse(s)
            }
        });
        let timestamp = at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        tx.execute(
            "INSERT INTO observations VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                input.id,
                input.target_id,
                input.source.as_str(),
                input.state.as_str(),
                timestamp,
                input.reason
            ],
        )
        .map_err(err)?;
        let mut kind = transition(prev, input.state);
        if input.state == StockState::Unknown {
            let recent: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE target_id=?1 AND source=?2 AND kind='HEALTH_ALERT' AND created_at>?3)",params![input.target_id,input.source.as_str(),(at-Duration::minutes(15)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true)],|r|r.get(0)).map_err(err)?;
            if !recent {
                kind = Some("HEALTH_ALERT");
            }
        }
        if let Some(kind) = kind {
            let event_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO events VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    event_id,
                    input.id,
                    input.target_id,
                    input.source.as_str(),
                    kind,
                    timestamp
                ],
            )
            .map_err(err)?;
            let notify = input.source != Source::Mock
                && match kind {
                    "FIRST_IN_STOCK" => settings.first_alert,
                    "RECOVERED_IN_STOCK" => settings.recovery_alert,
                    _ => true,
                };
            if notify {
                let feishu_target: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM target_notifications WHERE target_id=?1 AND feishu_enabled=1)", [&input.target_id], |r| r.get(0)).map_err(err)?;
                for (channel, enabled) in [
                    ("local", settings.local_notifications),
                    ("bark", settings.bark_enabled),
                    (
                        "feishu",
                        settings.feishu_enabled
                            && feishu_target
                            && input.state == StockState::InStock,
                    ),
                ] {
                    if enabled {
                        tx.execute("INSERT INTO deliveries(id,event_id,channel,status) VALUES(?1,?2,?3,'PENDING')",params![Uuid::new_v4().to_string(),event_id,channel]).map_err(err)?;
                    }
                }
            }
        }
        tx.commit().map_err(err)?;
        Ok(kind.map(str::to_string))
    }

    pub fn mock_tick(&mut self) -> Result<usize> {
        let settings = self.settings()?;
        if settings.mode != Source::Mock || settings.paused {
            return Err("请先切换到模拟模式并恢复运行".into());
        }
        let targets = self.targets(Source::Mock, Utc::now())?;
        let mut count = 0;
        for target in targets.into_iter().filter(|t| t.enabled) {
            let n: i64 = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM observations WHERE target_id=?1 AND source='mock'",
                    [&target.id],
                    |r| r.get(0),
                )
                .map_err(err)?;
            let state = [
                StockState::OutOfStock,
                StockState::InStock,
                StockState::InStock,
                StockState::Unknown,
                StockState::InStock,
                StockState::OutOfStock,
            ][n as usize % 6];
            self.record(
                &ObservationInput {
                    id: Uuid::new_v4().to_string(),
                    target_id: target.id,
                    source: Source::Mock,
                    state,
                    reason: (state == StockState::Unknown).then(|| "TIMEOUT".into()),
                },
                Utc::now(),
            )?;
            count += 1;
        }
        Ok(count)
    }

    pub fn history(
        &self,
        source: Source,
        days: i64,
        target: Option<&str>,
    ) -> Result<Vec<HistoryRow>> {
        let since = (Utc::now() - Duration::days(days.clamp(1, 180)))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.read_history(source, Some(&since), target, 5000)
    }

    fn read_history(
        &self,
        source: Source,
        since: Option<&str>,
        target: Option<&str>,
        limit: i64,
    ) -> Result<Vec<HistoryRow>> {
        let mut stmt = self.connection.prepare("SELECT o.id,o.target_id,v.json,s.json,o.state,o.observed_at,o.source,o.reason,e.kind FROM observations o JOIN targets t ON t.id=o.target_id JOIN variants v ON v.id=t.variant_id JOIN stores s ON s.id=t.store_key LEFT JOIN events e ON e.observation_id=o.id WHERE o.source=?1 AND (?2 IS NULL OR o.observed_at>=?2) AND (?3 IS NULL OR o.target_id=?3) ORDER BY o.observed_at DESC,o.rowid DESC LIMIT ?4").map_err(err)?;
        let rows = stmt
            .query_map(params![source.as_str(), since, target, limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(err)?;
        let mut result = Vec::new();
        for row in rows {
            let (id, target_id, p, s, state, observed_at, source, reason, event) =
                row.map_err(err)?;
            let p: Product = serde_json::from_str(&p).map_err(err)?;
            let s: Store = serde_json::from_str(&s).map_err(err)?;
            result.push(HistoryRow {
                id,
                target_id,
                product: format!("{} {} {}", p.model, p.capacity, p.color),
                store: format!("{} · {}", s.city, s.name),
                locale: p.locale,
                state,
                observed_at,
                source,
                reason,
                event,
            });
        }
        Ok(result)
    }

    pub fn summary(
        &self,
        source: Source,
        days: i64,
        target: Option<&str>,
        offset_minutes: i32,
    ) -> Result<Summary> {
        let since = (Utc::now() - Duration::days(days.clamp(1, 180)))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let offset =
            FixedOffset::east_opt(offset_minutes.clamp(-720, 840) * 60).ok_or("时区错误")?;
        let mut stmt=self.connection.prepare("SELECT state,observed_at FROM observations WHERE source=?1 AND observed_at>=?2 AND (?3 IS NULL OR target_id=?3) ORDER BY observed_at").map_err(err)?;
        let rows = stmt
            .query_map(params![source.as_str(), since, target], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(err)?;
        let mut cells: BTreeMap<(String, u32), HeatCell> = BTreeMap::new();
        let mut result = Summary {
            total: 0,
            valid: 0,
            in_stock: 0,
            events: 0,
            first_seen: None,
            last_seen: None,
            cells: Vec::new(),
        };
        for row in rows {
            let (state, time) = row.map_err(err)?;
            let local = DateTime::parse_from_rfc3339(&time)
                .map_err(err)?
                .with_timezone(&offset);
            let day = local.format("%Y-%m-%d").to_string();
            let cell = cells
                .entry((day.clone(), local.hour()))
                .or_insert(HeatCell {
                    day,
                    hour: local.hour(),
                    valid: 0,
                    in_stock: 0,
                    total: 0,
                });
            result.total += 1;
            cell.total += 1;
            if state != "UNKNOWN" {
                result.valid += 1;
                cell.valid += 1;
            }
            if state == "IN_STOCK" {
                result.in_stock += 1;
                cell.in_stock += 1;
                if result.first_seen.is_none() {
                    result.first_seen = Some(time.clone());
                }
                result.last_seen = Some(time);
            }
        }
        result.events=self.connection.query_row("SELECT COUNT(*) FROM events WHERE source=?1 AND created_at>=?2 AND kind!='HEALTH_ALERT' AND (?3 IS NULL OR target_id=?3)",params![source.as_str(),since,target],|r|r.get(0)).map_err(err)?;
        result.cells = cells.into_values().collect();
        Ok(result)
    }

    pub fn deliveries(&self) -> Result<Vec<Delivery>> {
        self.read_deliveries(false)
    }

    pub fn pending_deliveries(&self) -> Result<Vec<Delivery>> {
        self.read_deliveries(true)
    }

    fn read_deliveries(&self, pending_only: bool) -> Result<Vec<Delivery>> {
        let mut stmt=self.connection.prepare("SELECT d.id,e.target_id,e.kind,v.json,s.json,d.channel,d.status,d.attempted_at,d.error,e.source FROM deliveries d JOIN events e ON e.id=d.event_id JOIN targets t ON t.id=e.target_id JOIN variants v ON v.id=t.variant_id JOIN stores s ON s.id=t.store_key WHERE (?1=0 OR d.status='PENDING') ORDER BY e.created_at DESC LIMIT 100").map_err(err)?;
        let rows = stmt
            .query_map([pending_only], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, String>(9)?,
                ))
            })
            .map_err(err)?;
        let mut output = Vec::new();
        for row in rows {
            let (id, target_id, event, p, s, channel, status, attempted_at, error, source) =
                row.map_err(err)?;
            let p: Product = serde_json::from_str(&p).map_err(err)?;
            let s: Store = serde_json::from_str(&s).map_err(err)?;
            let prefix = match event.as_str() {
                "FIRST_IN_STOCK" => "首次记录可取货",
                "RECOVERED_IN_STOCK" => "恢复记录后可取货",
                "RESTOCK_DETECTED" => "检测到可取货",
                _ => "关注状态无法确认",
            };
            output.push(Delivery {
                id,
                target_id,
                event,
                title: format!("{prefix} · {}", p.model),
                body: format!(
                    "{} {} · {} / {}\n来源：{} · 请以 Apple 结账页为准",
                    p.capacity,
                    p.color,
                    s.city,
                    s.name,
                    if source == "apple" {
                        "Apple 商店查询"
                    } else {
                        "用户手动记录"
                    }
                ),
                url: p.official_url,
                channel,
                status,
                attempted_at,
                error,
                source: serde_json::from_value(serde_json::Value::String(source)).map_err(err)?,
            });
        }
        Ok(output)
    }

    pub fn claim_delivery(&self, id: &str) -> Result<bool> {
        Ok(self.connection.execute("UPDATE deliveries SET status='SENDING',attempted_at=?2 WHERE id=?1 AND status='PENDING'",params![id,now()]).map_err(err)?==1)
    }

    /// Recheck opt-in at claim time as well as observation time, under the app DB lock.
    pub fn claim_feishu_delivery(&self, id: &str, target_id: &str, source: Source) -> Result<bool> {
        let settings = self.settings()?;
        if !settings.feishu_enabled
            || settings.paused
            || settings.mode != source
            || source == Source::Mock
            || !self.feishu_target_enabled(target_id)?
        {
            return Ok(false);
        }
        Ok(self.connection.execute("UPDATE deliveries SET status='SENDING',attempted_at=?3 WHERE id=?1 AND channel='feishu' AND status='PENDING' AND event_id IN(SELECT id FROM events WHERE target_id=?2 AND source=?4)",params![id,target_id,now(),source.as_str()]).map_err(err)? == 1)
    }

    pub fn finish_delivery(&self, id: &str, result: std::result::Result<(), String>) -> Result<()> {
        let (status, error) = match result {
            Ok(()) => ("SENT", None),
            Err(e) => ("FAILED", Some(e)),
        };
        self.connection
            .execute(
                "UPDATE deliveries SET status=?2,error=?3 WHERE id=?1 AND status='SENDING'",
                params![id, status, error],
            )
            .map_err(err)?;
        Ok(())
    }

    pub fn prune(&self) -> Result<usize> {
        let days = self.settings()?.retention_days;
        let cutoff = (Utc::now() - Duration::days(days))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        // Keep latest and latest trustworthy sample per target/source so pruning never changes current state semantics.
        self.connection.execute("DELETE FROM observations WHERE observed_at<?1 AND rowid NOT IN (SELECT MAX(rowid) FROM observations GROUP BY target_id,source) AND rowid NOT IN (SELECT MAX(rowid) FROM observations WHERE state!='UNKNOWN' GROUP BY target_id,source)",[cutoff]).map_err(err)
    }

    pub fn export(&self, source: Source, format: &str) -> Result<String> {
        let history = self.read_history(source, None, None, -1)?;
        if format == "csv" {
            fn cell(s: &str) -> String {
                let safe = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                    format!("'{s}")
                } else {
                    s.into()
                };
                format!("\"{}\"", safe.replace('"', "\"\""))
            }
            let mut csv =
                "\u{feff}target_id,product,store,locale,state,observed_at,source,reason,event\r\n"
                    .to_string();
            for h in history {
                csv.push_str(
                    &[
                        h.target_id,
                        h.product,
                        h.store,
                        h.locale,
                        h.state,
                        h.observed_at,
                        h.source,
                        h.reason.unwrap_or_default(),
                        h.event.unwrap_or_default(),
                    ]
                    .iter()
                    .map(|s| cell(s))
                    .collect::<Vec<_>>()
                    .join(","),
                );
                csv.push_str("\r\n");
            }
            Ok(csv)
        } else if format == "json" {
            serde_json::to_string_pretty(&serde_json::json!({"schema_version":2,"source":source,"exported_at":now(),"observation_count":history.len(),"targets":self.targets(source,Utc::now())?,"observations":history,"deliveries":self.deliveries()?.into_iter().filter(|d|d.source==source).collect::<Vec<_>>()})).map_err(err)
        } else {
            Err("导出格式仅支持 csv/json".into())
        }
    }

    pub fn backup(&self, path: &Path) -> Result<()> {
        self.connection.backup("main", path, None).map_err(err)
    }

    pub fn restore(&mut self, path: &Path) -> Result<()> {
        let check = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(err)?;
        let integrity: String = check
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(err)?;
        if integrity != "ok" {
            return Err("备份数据库校验失败".into());
        }
        let version: i64 = check
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if ![1, 2, 3].contains(&version) {
            return Err("备份版本不兼容".into());
        }
        for table in [
            "targets",
            "variants",
            "stores",
            "settings",
            "observations",
            "events",
            "deliveries",
            "watch_groups",
            "group_members",
        ] {
            let exists: bool = check
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if !exists {
                return Err("备份缺少必要数据表".into());
            }
        }
        let violations: i64 = check
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .map_err(err)?;
        if violations != 0 {
            return Err("备份外键校验失败".into());
        }
        // Validate a temporary copy using the same read paths, before replacing the live database.
        let mut candidate = Database::memory()?;
        candidate
            .connection
            .restore("main", path, None::<fn(rusqlite::backup::Progress)>)
            .map_err(err)?;
        if version == 1 {
            Self::migrate_v1(&candidate.connection)?;
        }
        Self::migrate_feishu(&candidate.connection)?;
        candidate.settings()?;
        let catalog = Catalog::load()?;
        let mut variants = candidate
            .connection
            .prepare("SELECT id,locale,json FROM variants")
            .map_err(err)?;
        for row in variants
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(err)?
        {
            let (id, locale, json) = row.map_err(err)?;
            let p: Product =
                serde_json::from_str(&json).map_err(|_| "备份商品数据无效".to_string())?;
            let expected = catalog.product(&id)?;
            if p.id != id
                || p.locale != locale
                || serde_json::to_value(&p).map_err(err)?
                    != serde_json::to_value(expected).map_err(err)?
            {
                return Err("备份商品与当前目录不符".into());
            }
            official_url(&p.official_url)?;
        }
        let mut stores = candidate
            .connection
            .prepare("SELECT id,locale,json FROM stores")
            .map_err(err)?;
        for row in stores
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(err)?
        {
            let (key, locale, json) = row.map_err(err)?;
            let s: Store =
                serde_json::from_str(&json).map_err(|_| "备份门店数据无效".to_string())?;
            let expected = catalog.store(&locale, &s.id)?;
            if key != format!("{}:{}", locale, s.id)
                || s.locale != locale
                || serde_json::to_value(&s).map_err(err)?
                    != serde_json::to_value(expected).map_err(err)?
            {
                return Err("备份门店与当前目录不符".into());
            }
        }
        let invalid: i64 = candidate.connection.query_row("SELECT
            (SELECT COUNT(*) FROM targets t JOIN variants v ON t.variant_id=v.id JOIN stores s ON t.store_key=s.id WHERE v.locale!=s.locale OR t.priority NOT BETWEEN 0 AND 2 OR t.enabled NOT IN (0,1) OR t.archived NOT IN (0,1)) +
            (SELECT COUNT(*) FROM observations WHERE source NOT IN ('manual','mock','apple') OR state NOT IN ('IN_STOCK','OUT_OF_STOCK','UNKNOWN')) +
            (SELECT COUNT(*) FROM deliveries WHERE channel NOT IN ('local','bark','feishu') OR status NOT IN ('PENDING','SENDING','SENT','FAILED','UNCERTAIN','CANCELLED')) +
            (SELECT COUNT(*) FROM target_notifications WHERE feishu_enabled NOT IN (0,1))",[],|r|r.get(0)).map_err(err)?;
        if invalid != 0 {
            return Err("备份包含无效记录".into());
        }
        candidate.groups()?;
        candidate.deliveries()?;
        for source in [Source::Manual, Source::Mock, Source::Apple] {
            candidate.targets(source, Utc::now())?;
            candidate.read_history(source, None, None, -1)?;
            candidate.summary(source, 180, None, 0)?;
        }
        drop(check);
        let protection = self.settings()?;
        let last_request = protection.scan_last_request_at;
        rusqlite::backup::Backup::new(&candidate.connection, &mut self.connection)
            .map_err(err)?
            .run_to_completion(100, StdDuration::from_millis(0), None)
            .map_err(err)?;
        self.connection.execute("UPDATE deliveries SET status='UNCERTAIN',error='从备份恢复后未自动重发' WHERE status IN ('PENDING','SENDING')",[]).map_err(err)?;
        let mut settings = self.settings()?;
        settings.bark_enabled = false;
        settings.feishu_enabled = false;
        settings.quiesce_scan_alert();
        settings.mock_running = false;
        settings.scan_running = false;
        settings.scan_connection_protection |= protection.scan_connection_protection;
        let at = Utc::now();
        let current_budget = protection.request_budget(at).projected(at);
        let restored_budget = settings.request_budget(at).projected(at);
        settings.scan_budget = Some(crate::request_budget::RequestBudget {
            credit_ms: current_budget.credit_ms.min(restored_budget.credit_ms),
            updated_at: current_budget.updated_at.max(restored_budget.updated_at),
        });
        settings.scan_retry_after = settings
            .scan_retry_after
            .into_iter()
            .chain(protection.scan_retry_after)
            .chain(protection.resume_after())
            .max();
        settings.scan_failure_count = settings
            .scan_failure_count
            .max(protection.scan_failure_count);
        if protection.scan_stop_reason.is_some() {
            settings.scan_stop_reason = protection.scan_stop_reason;
        }
        settings.scan_invalidated_at = settings
            .scan_invalidated_at
            .max(protection.scan_invalidated_at);
        settings.scan_last_request_at = settings.scan_last_request_at.max(last_request);
        self.save_settings(&settings)?;
        Ok(())
    }

    pub fn clear(&mut self, source: Source) -> Result<()> {
        let tx = self.connection.transaction().map_err(err)?;
        tx.execute(
            "DELETE FROM observations WHERE source=?1",
            [source.as_str()],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(())
    }
}
