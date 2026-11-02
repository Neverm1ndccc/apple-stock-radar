use crate::{Result, Settings, Source, apple::ScanFailure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlertStatus {
    Pending,
    Sending,
    Sent,
    Failed,
    Cancelled,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanAlert {
    pub id: String,
    pub at: DateTime<Utc>,
    pub reason: String,
    pub http_status: Option<u16>,
    pub status: AlertStatus,
    pub error: Option<String>,
}

impl ScanAlert {
    pub fn validate(&self) -> bool {
        self.id.len() == 36
            && matches!(
                self.reason.as_str(),
                "BLOCKED" | "RATE_LIMITED" | "UNAUTHORIZED" | "PARSE_ERROR" | "STORAGE_ERROR"
            )
            && self.http_status.is_none_or(|s| (100..=599).contains(&s))
            && self.error.as_ref().is_none_or(|e| e.len() <= 400)
    }
    pub fn message(&self) -> (String, String) {
        let reason = match self.reason.as_str() {
            "BLOCKED" => "Apple 来源拒绝访问",
            "RATE_LIMITED" => "Apple 来源限流",
            "UNAUTHORIZED" => "来源访问配置不可用",
            "STORAGE_ERROR" => "本地记录写入失败",
            _ => "库存响应无法解析",
        };
        let at = self
            .at
            .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap());
        (
            "库存雷达扫描已停止".into(),
            format!(
                "{reason}{}\n时间：{}（北京时间）\n已停止后续请求，当前库存无法确认。请在应用设置中检查原因，保护等待结束后手动恢复。",
                self.http_status
                    .map(|s| format!("（HTTP {s}）"))
                    .unwrap_or_default(),
                at.format("%Y-%m-%d %H:%M:%S")
            ),
        )
    }
}

impl Settings {
    pub fn queue_scan_alert(&mut self, at: DateTime<Utc>, failure: ScanFailure, http: Option<u16>) {
        if !failure.stops_source()
            || self.mode != Source::Apple
            || self.paused
            || !self.scan_failure_notifications
            || !self.feishu_enabled
        {
            return;
        }
        if self
            .scan_alert
            .as_ref()
            .is_some_and(|a| a.at == at && a.reason == failure.reason())
        {
            return;
        }
        self.scan_alert = Some(ScanAlert {
            id: uuid::Uuid::new_v4().to_string(),
            at,
            reason: failure.reason().into(),
            http_status: http,
            status: AlertStatus::Pending,
            error: None,
        });
    }
    pub fn claim_scan_alert(&mut self) -> Option<ScanAlert> {
        if !self.feishu_enabled
            || !self.scan_failure_notifications
            || self.paused
            || self.mode != Source::Apple
        {
            return None;
        }
        let alert = self.scan_alert.as_mut()?;
        if alert.status != AlertStatus::Pending {
            return None;
        }
        alert.status = AlertStatus::Sending;
        Some(alert.clone())
    }
    pub fn finish_scan_alert(&mut self, id: &str, result: Result<()>) {
        if let Some(alert) = self
            .scan_alert
            .as_mut()
            .filter(|a| a.id == id && a.status == AlertStatus::Sending)
        {
            match result {
                Ok(()) => {
                    alert.status = AlertStatus::Sent;
                    alert.error = None;
                }
                Err(error) => {
                    alert.status = AlertStatus::Failed;
                    alert.error = Some(error.chars().take(100).collect());
                }
            }
        }
    }
    pub fn cancel_scan_alert(&mut self) {
        if let Some(alert) = self
            .scan_alert
            .as_mut()
            .filter(|a| a.status == AlertStatus::Pending)
        {
            alert.status = AlertStatus::Cancelled;
        }
    }
    /// Restart/restore must not replay an old or uncertain external notification.
    pub fn quiesce_scan_alert(&mut self) -> bool {
        match self.scan_alert.as_mut() {
            Some(alert) if alert.status == AlertStatus::Pending => {
                alert.status = AlertStatus::Cancelled;
                true
            }
            Some(alert) if alert.status == AlertStatus::Sending => {
                alert.status = AlertStatus::Uncertain;
                true
            }
            _ => false,
        }
    }
}
