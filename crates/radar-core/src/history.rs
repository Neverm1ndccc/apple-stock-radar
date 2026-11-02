use crate::{Database, HeatCell, HistoryRow, Product, Result, Source, StockState, Store, Summary};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params_from_iter, types::Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryFilter {
    pub days: Option<i64>,
    pub target_id: Option<String>,
    pub state: Option<StockState>,
    pub locale: Option<String>,
    pub query: String,
    pub event: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HistoryResult {
    pub rows: Vec<HistoryRow>,
    pub summary: Summary,
    pub since: String,
    pub until: String,
}

impl Database {
    pub fn query_history(
        &self,
        source: Source,
        filter: &HistoryFilter,
        offset: i32,
    ) -> Result<HistoryResult> {
        let error = |e: rusqlite::Error| e.to_string();
        let parse = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .map(|at| at.with_timezone(&Utc))
                .map_err(|_| "历史时间格式无效".to_string())
        };
        let until = filter
            .to
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_else(Utc::now);
        let since = filter
            .from
            .as_deref()
            .map(parse)
            .transpose()?
            .map(Ok)
            .unwrap_or_else(|| {
                until
                    .checked_sub_signed(Duration::days(filter.days.unwrap_or(7).clamp(1, 180)))
                    .ok_or_else(|| "历史时间超出范围".to_string())
            })?;
        if since > until || until - since > Duration::days(180) {
            return Err("开始时间须不晚于结束时间，范围最多180天".into());
        }
        if filter.query.chars().count() > 200 || filter.query.split_whitespace().count() > 12 {
            return Err("关键字最多200字、12个词".into());
        }
        let since = since.to_rfc3339_opts(SecondsFormat::Millis, true);
        let until = until.to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut values = vec![
            Value::Text(source.as_str().into()),
            Value::Text(since.clone()),
            Value::Text(until.clone()),
        ];
        let mut conditions = vec![
            "o.source=?1".to_string(),
            "o.observed_at>=?2".into(),
            "o.observed_at<=?3".into(),
        ];
        let mut bind = |condition: &str, value: String| {
            values.push(Value::Text(value));
            conditions.push(format!("{condition}?{}", values.len()));
        };
        if let Some(id) = &filter.target_id {
            bind("o.target_id=", id.clone());
        }
        if let Some(state) = filter.state {
            bind("o.state=", state.as_str().into());
        }
        if let Some(locale) = &filter.locale {
            if !["zh_CN", "zh_HK"].contains(&locale.as_str()) {
                return Err("历史地区无效".into());
            }
            bind("v.locale=", locale.clone());
        }
        if let Some(event) = &filter.event {
            if event != "EVENTS_ONLY"
                && ![
                    "FIRST_IN_STOCK",
                    "RECOVERED_IN_STOCK",
                    "RESTOCK_DETECTED",
                    "HEALTH_ALERT",
                ]
                .contains(&event.as_str())
            {
                return Err("历史事件筛选无效".into());
            }
            if event != "EVENTS_ONLY" {
                bind("e.kind=", event.clone());
            }
        }
        for word in filter.query.split_whitespace() {
            values.push(Value::Text(word.into()));
            conditions.push(format!("instr(lower(json_extract(v.json,'$.model') || ' ' || json_extract(v.json,'$.capacity') || ' ' || json_extract(v.json,'$.color') || ' ' || json_extract(v.json,'$.sku') || ' ' || json_extract(s.json,'$.city') || ' ' || json_extract(s.json,'$.name')),lower(?{}))>0", values.len()));
        }
        if filter.event.as_deref() == Some("EVENTS_ONLY") {
            conditions.push("e.kind IS NOT NULL".into());
        }
        let scope = format!(
            "FROM observations o JOIN targets t ON t.id=o.target_id JOIN variants v ON v.id=t.variant_id JOIN stores s ON s.id=t.store_key LEFT JOIN events e ON e.observation_id=o.id WHERE {}",
            conditions.join(" AND ")
        );
        let mut statement = self.connection.prepare(&format!("SELECT o.id,o.target_id,v.json,s.json,o.state,o.observed_at,o.source,o.reason,e.kind {scope} ORDER BY o.observed_at DESC,o.rowid DESC LIMIT 5000")).map_err(error)?;
        let rows = statement
            .query_map(params_from_iter(&values), |r| {
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
            .map_err(error)?;
        let mut output = Vec::new();
        for row in rows {
            let (id, target_id, p, s, state, observed_at, source, reason, event) =
                row.map_err(error)?;
            let p: Product = serde_json::from_str(&p).map_err(|e| e.to_string())?;
            let s: Store = serde_json::from_str(&s).map_err(|e| e.to_string())?;
            output.push(HistoryRow {
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
        let mut summary = self.connection.query_row(&format!("SELECT COUNT(*),COALESCE(SUM(o.state!='UNKNOWN'),0),COALESCE(SUM(o.state='IN_STOCK'),0),COALESCE(SUM(e.kind IN ('FIRST_IN_STOCK','RECOVERED_IN_STOCK','RESTOCK_DETECTED')),0),MIN(CASE WHEN o.state='IN_STOCK' THEN o.observed_at END),MAX(CASE WHEN o.state='IN_STOCK' THEN o.observed_at END) {scope}"),params_from_iter(&values),|r| Ok(Summary {total:r.get(0)?,valid:r.get(1)?,in_stock:r.get(2)?,events:r.get(3)?,first_seen:r.get(4)?,last_seen:r.get(5)?,cells:Vec::new()})).map_err(error)?;
        values.push(Value::Text(format!(
            "{:+} minutes",
            offset.clamp(-720, 840)
        )));
        let offset_parameter = values.len();
        let mut statement = self.connection.prepare(&format!("SELECT strftime('%Y-%m-%d',o.observed_at,?{offset_parameter}),CAST(strftime('%H',o.observed_at,?{offset_parameter}) AS INTEGER),COUNT(*),SUM(o.state!='UNKNOWN'),SUM(o.state='IN_STOCK') {scope} GROUP BY 1,2 ORDER BY 1,2")).map_err(error)?;
        summary.cells = statement
            .query_map(params_from_iter(&values), |r| {
                Ok(HeatCell {
                    day: r.get(0)?,
                    hour: r.get(1)?,
                    total: r.get(2)?,
                    valid: r.get(3)?,
                    in_stock: r.get(4)?,
                })
            })
            .map_err(error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        Ok(HistoryResult {
            rows: output,
            summary,
            since,
            until,
        })
    }
}
