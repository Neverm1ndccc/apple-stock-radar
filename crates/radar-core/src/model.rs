use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    pub id: String,
    pub locale: String,
    pub category: String,
    pub model: String,
    pub capacity: String,
    pub color: String,
    pub sku: String,
    pub official_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub id: String,
    pub locale: String,
    pub city: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StockState {
    InStock,
    OutOfStock,
    Unknown,
}

impl StockState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InStock => "IN_STOCK",
            Self::OutOfStock => "OUT_OF_STOCK",
            Self::Unknown => "UNKNOWN",
        }
    }
    pub fn parse(value: &str) -> Self {
        match value {
            "IN_STOCK" => Self::InStock,
            "OUT_OF_STOCK" => Self::OutOfStock,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Manual,
    Mock,
    Apple,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Mock => "mock",
            Self::Apple => "apple",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub mode: Source,
    pub paused: bool,
    pub stale_minutes: i64,
    pub retention_days: i64,
    pub local_notifications: bool,
    pub bark_enabled: bool,
    #[serde(default)]
    pub feishu_enabled: bool,
    pub first_alert: bool,
    pub recovery_alert: bool,
    pub mock_running: bool,
    pub mock_interval_seconds: u64,
    #[serde(default)]
    pub scan_running: bool,
    #[serde(default = "default_scan_interval")]
    pub scan_interval_seconds: u64,
    #[serde(default)]
    pub scan_last_request_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub apple_personal_access: bool,
    #[serde(default)]
    pub scan_stop_reason: Option<String>,
    #[serde(default)]
    pub scan_invalidated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub scan_last_http_status: Option<u16>,
    #[serde(default)]
    pub scan_budget: Option<crate::request_budget::RequestBudget>,
    #[serde(default)]
    pub scan_retry_after: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub scan_failure_count: u32,
    #[serde(default)]
    pub scan_connection_protection: bool,
    #[serde(default)]
    pub scan_failure_notifications: bool,
    #[serde(default)]
    pub scan_alert: Option<crate::scan_health::ScanAlert>,
    #[serde(default)]
    pub scan_last_http_version: Option<String>,
    #[serde(default)]
    pub scan_last_duration_ms: Option<u64>,
}

fn default_scan_interval() -> u64 {
    300
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Source::Manual,
            paused: false,
            stale_minutes: 10,
            retention_days: 90,
            local_notifications: false,
            bark_enabled: false,
            feishu_enabled: false,
            first_alert: true,
            recovery_alert: true,
            mock_running: false,
            mock_interval_seconds: 60,
            scan_running: false,
            scan_interval_seconds: default_scan_interval(),
            scan_last_request_at: None,
            apple_personal_access: false,
            scan_stop_reason: None,
            scan_invalidated_at: None,
            scan_last_http_status: None,
            scan_budget: None,
            scan_retry_after: None,
            scan_failure_count: 0,
            scan_connection_protection: false,
            scan_failure_notifications: false,
            scan_alert: None,
            scan_last_http_version: None,
            scan_last_duration_ms: None,
        }
    }
}

impl Settings {
    pub fn user_update(
        mut self,
        previous: &Settings,
        source_available: bool,
    ) -> crate::Result<Self> {
        self.scan_running = previous.scan_running && self.mode == Source::Apple && source_available;
        self.scan_last_request_at = previous.scan_last_request_at;
        self.scan_stop_reason = previous.scan_stop_reason.clone();
        self.scan_invalidated_at = previous.scan_invalidated_at;
        self.scan_last_http_status = previous.scan_last_http_status;
        self.scan_budget = previous.scan_budget.clone();
        self.scan_retry_after = previous.scan_retry_after;
        self.scan_failure_count = previous.scan_failure_count;
        self.feishu_enabled = previous.feishu_enabled;
        self.scan_alert = previous.scan_alert.clone();
        self.scan_last_http_version = previous.scan_last_http_version.clone();
        self.scan_last_duration_ms = previous.scan_last_duration_ms;
        if !self.scan_failure_notifications || self.paused || self.mode != Source::Apple {
            self.cancel_scan_alert();
        }
        self.validate()?;
        Ok(self)
    }
    pub fn set_scanning(&mut self, enabled: bool, source_available: bool) -> crate::Result<()> {
        self.set_scanning_at(enabled, source_available, chrono::Utc::now())
    }
    pub fn set_scanning_at(
        &mut self,
        enabled: bool,
        source_available: bool,
        now: chrono::DateTime<chrono::Utc>,
    ) -> crate::Result<()> {
        if enabled && (self.mode != Source::Apple || !source_available) {
            return Err("请先允许 Apple 个人查询并切换到 Apple 模式".into());
        }
        if enabled && self.resume_after().is_some_and(|at| at > now) {
            return Err("来源仍处于保护等待期，请在设置中显示的时间后手动恢复".into());
        }
        self.scan_running = enabled;
        if enabled {
            self.cancel_scan_alert();
            if self.scan_stop_reason.is_some() {
                self.scan_failure_count = self.scan_failure_count.max(1);
            }
            self.scan_stop_reason = None;
        }
        self.validate()
    }
    pub fn request_budget(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> crate::request_budget::RequestBudget {
        self.scan_budget.clone().unwrap_or_else(|| {
            self.scan_last_request_at
                .map(crate::request_budget::RequestBudget::empty)
                .unwrap_or_else(|| crate::request_budget::RequestBudget::full(now))
        })
    }
    pub fn scan_minimum_interval(&self, provider_minimum: u64) -> u64 {
        provider_minimum.max(if self.scan_connection_protection {
            180
        } else {
            0
        })
    }
    pub fn effective_scan_interval(&self, provider_minimum: u64) -> u64 {
        self.scan_interval_seconds
            .max(self.scan_minimum_interval(provider_minimum))
    }
    pub fn budget_ready_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match &self.scan_budget {
            Some(b) => b.ready_at(),
            None => self.scan_last_request_at.map(|at| {
                at + chrono::Duration::seconds(crate::request_budget::REFILL_SECONDS as i64)
            }),
        }
    }
    pub fn resume_after(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        // Temporary network/service backoff delays requests, not the user's
        // ability to enable scheduling. Only a stopped source blocks recovery.
        self.scan_stop_reason.as_ref()?;
        let protection = if matches!(
            self.scan_stop_reason.as_deref(),
            Some("BLOCKED" | "RATE_LIMITED" | "UNAUTHORIZED")
        ) {
            self.scan_invalidated_at
                .or(self.scan_last_request_at)
                .map(|at| {
                    at + chrono::Duration::minutes(
                        [5, 10, 20, 30][self.scan_failure_count.saturating_sub(1).min(3) as usize],
                    )
                })
        } else {
            None
        };
        self.scan_retry_after.into_iter().chain(protection).max()
    }
    pub fn apply_scan_result(
        &mut self,
        at: chrono::DateTime<chrono::Utc>,
        minimum: u64,
        failure: Option<crate::apple::ScanFailure>,
        retry: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        self.scan_last_request_at = Some(at);
        if let Some(failure) = failure {
            self.scan_failure_count = self.scan_failure_count.saturating_add(1);
            if failure.stops_source() {
                self.scan_running = false;
                self.scan_stop_reason = Some(failure.reason().into());
                self.scan_invalidated_at = Some(at);
                self.scan_retry_after = None;
                self.scan_retry_after = self.resume_after().into_iter().chain(retry).max();
            } else {
                let delay = self
                    .effective_scan_interval(minimum)
                    .saturating_mul(1 << self.scan_failure_count.min(8))
                    .min(86400)
                    .max(minimum);
                self.scan_retry_after = Some(at + chrono::Duration::seconds(delay as i64))
                    .into_iter()
                    .chain(retry)
                    .max();
            }
        } else {
            self.scan_failure_count = 0;
            self.scan_retry_after = None;
        }
    }
    pub fn validate(&self) -> crate::Result<()> {
        if !(5..=60).contains(&self.stale_minutes)
            || ![7, 30, 90, 180].contains(&self.retention_days)
            || !(30..=3600).contains(&self.mock_interval_seconds)
            || !(5..=86400).contains(&self.scan_interval_seconds)
            || self.scan_budget.as_ref().is_some_and(|b| !b.validate())
            || self.scan_alert.as_ref().is_some_and(|a| !a.validate())
            || self.scan_last_http_version.as_deref().is_some_and(|v| {
                !["HTTP/0.9", "HTTP/1.0", "HTTP/1.1", "HTTP/2", "HTTP/3"].contains(&v)
            })
            || self
                .scan_invalidated_at
                .is_some_and(|at| at > chrono::Utc::now() + chrono::Duration::minutes(1))
            || self
                .scan_last_request_at
                .is_some_and(|at| at > chrono::Utc::now() + chrono::Duration::minutes(1))
            || self.scan_stop_reason.as_deref().is_some_and(|reason| {
                ![
                    "UNAUTHORIZED",
                    "BLOCKED",
                    "RATE_LIMITED",
                    "PARSE_ERROR",
                    "STORAGE_ERROR",
                ]
                .contains(&reason)
            })
        {
            return Err("设置超出允许范围".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub product: Product,
    pub store: Store,
    pub enabled: bool,
    pub priority: i64,
    pub feishu_enabled: bool,
    pub state: String,
    pub last_state: Option<String>,
    pub observed_at: Option<String>,
    pub last_valid_at: Option<String>,
    pub reason: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetBatchAction {
    EnableFeishu,
    DisableFeishu,
    Pause,
    Resume,
    HighPriority,
    NormalPriority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationInput {
    pub id: String,
    pub target_id: String,
    pub source: Source,
    pub state: StockState,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRow {
    pub id: String,
    pub target_id: String,
    pub product: String,
    pub store: String,
    pub locale: String,
    pub state: String,
    pub observed_at: String,
    pub source: String,
    pub reason: Option<String>,
    pub event: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub target_id: String,
    pub event: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub channel: String,
    pub status: String,
    pub attempted_at: Option<String>,
    pub error: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatCell {
    pub day: String,
    pub hour: u32,
    pub valid: u64,
    pub in_stock: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub total: u64,
    pub valid: u64,
    pub in_stock: u64,
    pub events: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub cells: Vec<HeatCell>,
}

#[derive(Debug, Serialize)]
pub struct AddResult {
    pub added: usize,
    pub duplicates: usize,
    pub combinations: usize,
}

#[derive(Debug, Serialize)]
pub struct WatchGroup {
    pub id: String,
    pub title: String,
    pub target_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub targets: Vec<Target>,
    pub settings: Settings,
    pub deliveries: Vec<Delivery>,
    pub groups: Vec<WatchGroup>,
    pub catalog_version: String,
    pub automatic_available: bool,
    pub scanner: crate::scanner::ScanStatus,
}

/// Unknown interrupts a trustworthy interval; recovery is not evidence of a restock.
pub fn transition(previous: Option<StockState>, next: StockState) -> Option<&'static str> {
    if next != StockState::InStock {
        return None;
    }
    match previous {
        None => Some("FIRST_IN_STOCK"),
        Some(StockState::Unknown) => Some("RECOVERED_IN_STOCK"),
        Some(StockState::OutOfStock) => Some("RESTOCK_DETECTED"),
        Some(StockState::InStock) => None,
    }
}
