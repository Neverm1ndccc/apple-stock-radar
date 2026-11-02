//! Offline-tested adapter for the upstream-reported Apple pickup response contract.
//! Transport requires a provider grant or the user's explicit personal-use opt-in.
use crate::{ObservationInput, Result, Source, StockState, Target};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppleAuthorization {
    pub mechanism: String,
    pub permission_reference: String,
    pub allowed_locales: Vec<String>,
    pub minimum_interval_seconds: u64,
    pub expires_at: DateTime<Utc>,
}

impl AppleAuthorization {
    pub fn validate(&self, now: DateTime<Utc>) -> Result<()> {
        if self.mechanism != "apple-pickup-message-v1"
            || self.permission_reference.trim().len() < 8
            || self.permission_reference.len() > 500
            || self.allowed_locales.is_empty()
            || self
                .allowed_locales
                .iter()
                .any(|l| !["zh_CN", "zh_HK"].contains(&l.as_str()))
            || !(5..=86400).contains(&self.minimum_interval_seconds)
            || self.expires_at <= now
        {
            return Err("Apple 来源许可记录无效或已过期；自动请求未启用".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum AppleAccess {
    Disabled,
    Personal,
    Licensed(AppleAuthorization),
}

impl AppleAccess {
    pub fn configured(personal: bool, grant: Option<&AppleAuthorization>) -> Self {
        match grant {
            Some(grant) => Self::Licensed(grant.clone()),
            None if personal => Self::Personal,
            None => Self::Disabled,
        }
    }
    pub fn validate(&self, now: DateTime<Utc>) -> Result<()> {
        match self {
            Self::Disabled => Err("尚未启用 Apple 个人查询，也没有有效的来源许可记录".into()),
            Self::Personal => Ok(()),
            Self::Licensed(grant) => grant.validate(now),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Personal => "personal",
            Self::Licensed(_) => "licensed",
        }
    }
    pub fn minimum_interval(&self) -> u64 {
        match self {
            Self::Licensed(g) => g.minimum_interval_seconds,
            _ => 5,
        }
    }
    pub fn locales(&self) -> Vec<String> {
        match self {
            Self::Disabled => vec![],
            Self::Personal => vec!["zh_CN".into(), "zh_HK".into()],
            Self::Licensed(g) => g.allowed_locales.clone(),
        }
    }
}

pub fn accessible_request(
    access: &AppleAccess,
    batch: &AppleBatch,
    now: DateTime<Utc>,
) -> Result<url::Url> {
    access.validate(now)?;
    if !access.locales().contains(&batch.locale) {
        return Err("该地区不在来源访问范围内".into());
    }
    batch.request_url()
}

pub fn authorized_request(
    grant: Option<&AppleAuthorization>,
    batch: &AppleBatch,
    now: DateTime<Utc>,
) -> Result<url::Url> {
    accessible_request(&AppleAccess::configured(false, grant), batch, now)
}

#[derive(Debug, Clone)]
pub struct AppleBatch {
    pub locale: String,
    pub store: String,
    pub targets: Vec<Target>,
}

pub fn batches(targets: Vec<Target>) -> Vec<AppleBatch> {
    let mut grouped: BTreeMap<(String, String), Vec<Target>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for t in targets.into_iter().filter(|t| t.enabled) {
        if seen.insert(t.id.clone()) {
            grouped
                .entry((t.product.locale.clone(), t.store.id.clone()))
                .or_default()
                .push(t);
        }
    }
    grouped
        .into_iter()
        .flat_map(|((locale, store), targets)| {
            targets
                .chunks(20)
                .map(|chunk| AppleBatch {
                    locale: locale.clone(),
                    store: store.clone(),
                    targets: chunk.to_vec(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

impl AppleBatch {
    /// Reuse only explicitly returned, exact SKU/store results for other watched
    /// targets. Missing nearby stores/SKUs are not observations of those targets.
    pub fn parse_coverage(
        &self,
        raw: &[u8],
        watched: &[Target],
    ) -> std::result::Result<Vec<ObservationInput>, ScanFailure> {
        let value = parse_payload(raw)?;
        let mut output = self.parse_value(&value)?;
        let primary_ids: HashSet<_> = self.targets.iter().map(|t| t.id.as_str()).collect();
        let requested_parts: HashSet<_> = self
            .targets
            .iter()
            .map(|t| t.product.sku.as_str())
            .collect();
        let stores = value
            .pointer("/body/stores")
            .or_else(|| value.pointer("/body/content/pickupMessage/stores"))
            .and_then(Value::as_array)
            .ok_or(ScanFailure::Parse)?;
        let related: Vec<_> = watched
            .iter()
            .filter(|target| {
                target.enabled
                    && target.product.locale == self.locale
                    && target.store.locale == self.locale
                    && !primary_ids.contains(target.id.as_str())
                    && requested_parts.contains(target.product.sku.as_str())
                    && stores.iter().any(|store| {
                        store.get("storeNumber").and_then(Value::as_str)
                            == Some(target.store.id.as_str())
                            && store
                                .get("partsAvailability")
                                .and_then(|parts| parts.get(&target.product.sku))
                                .is_some()
                    })
            })
            .cloned()
            .collect();
        for batch in batches(related) {
            output.extend(batch.parse_value(&value)?);
        }
        Ok(output)
    }
    pub fn request_url(&self) -> Result<url::Url> {
        if self.targets.is_empty()
            || self.targets.len() > 20
            || self.targets.iter().any(|t| {
                t.product.locale != self.locale
                    || t.store.locale != self.locale
                    || t.store.id != self.store
            })
        {
            return Err("Apple 查询批次地区或门店不一致".into());
        }
        let base = match self.locale.as_str() {
            "zh_CN" => "https://www.apple.com.cn/shop/retail/pickup-message",
            "zh_HK" => "https://www.apple.com/hk-zh/shop/retail/pickup-message",
            _ => return Err("地区不支持".into()),
        };
        let mut url = url::Url::parse(base).map_err(|_| "来源地址无效")?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("pl", "true")
                .append_pair("mts.0", "regular")
                .append_pair("store", &self.store);
            for (index, target) in self.targets.iter().enumerate() {
                query.append_pair(&format!("parts.{index}"), &target.product.sku);
            }
        }
        Ok(url)
    }

    pub fn unknown(&self, reason: &str) -> Vec<ObservationInput> {
        self.targets
            .iter()
            .map(|t| ObservationInput {
                id: uuid::Uuid::new_v4().to_string(),
                target_id: t.id.clone(),
                source: Source::Apple,
                state: StockState::Unknown,
                reason: Some(reason.into()),
            })
            .collect()
    }

    pub fn parse(&self, raw: &[u8]) -> std::result::Result<Vec<ObservationInput>, ScanFailure> {
        let value = parse_payload(raw)?;
        self.parse_value(&value)
    }

    fn parse_value(
        &self,
        value: &Value,
    ) -> std::result::Result<Vec<ObservationInput>, ScanFailure> {
        let stores = value
            .pointer("/body/stores")
            .and_then(Value::as_array)
            .or_else(|| {
                value
                    .pointer("/body/content/pickupMessage/stores")
                    .and_then(Value::as_array)
            })
            .ok_or(ScanFailure::Parse)?;
        let matches: Vec<_> = stores
            .iter()
            .filter(|s| s.get("storeNumber").and_then(Value::as_str) == Some(&self.store))
            .collect();
        if matches.len() > 1 {
            return Err(ScanFailure::Parse);
        }
        let mut output = self.unknown("NO_MATCH");
        let Some(store) = matches.first() else {
            return Ok(output);
        };
        let parts = store
            .get("partsAvailability")
            .and_then(Value::as_object)
            .ok_or(ScanFailure::Parse)?;
        for (target, input) in self.targets.iter().zip(output.iter_mut()) {
            let Some(part) = parts.get(&target.product.sku) else {
                continue;
            };
            if part
                .get("partNumber")
                .is_some_and(|p| p.as_str() != Some(&target.product.sku))
            {
                input.reason = Some("PARSE_ERROR".into());
                continue;
            }
            let (state, reason) = match part.get("pickupDisplay").and_then(Value::as_str) {
                Some("available") => (StockState::InStock, None),
                Some("unavailable") => (StockState::OutOfStock, None),
                Some("ineligible") => (StockState::Unknown, Some("UNSUPPORTED".into())),
                _ => (StockState::Unknown, Some("PARSE_ERROR".into())),
            };
            input.state = state;
            input.reason = reason;
        }
        Ok(output)
    }
}

/// Prefer the batch containing the oldest observed target. This prevents asking
/// again for stores already covered by a nearby response and survives restart.
pub fn next_batch_index(batches: &[AppleBatch], cursor: usize) -> Option<usize> {
    if batches.is_empty() {
        return None;
    }
    (0..batches.len())
        .map(|offset| (cursor % batches.len() + offset) % batches.len())
        .min_by_key(|index| {
            batches[*index]
                .targets
                .iter()
                .map(|target| {
                    target
                        .observed_at
                        .as_deref()
                        .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                        .map(|at| at.with_timezone(&Utc))
                })
                .min()
                .flatten()
        })
}

fn parse_payload(raw: &[u8]) -> std::result::Result<Value, ScanFailure> {
    let prefix = String::from_utf8_lossy(&raw[..raw.len().min(512)]).to_lowercase();
    let prefix = prefix.trim_start_matches('\u{feff}').trim_start();
    if prefix.starts_with('<') && (prefix.contains("<html") || prefix.contains("<!doctype html")) {
        return Err(ScanFailure::Blocked);
    }
    let value: Value = serde_json::from_slice(raw).map_err(|_| ScanFailure::Parse)?;
    if let Some(status) = value.pointer("/head/status") {
        let code = status
            .as_u64()
            .or_else(|| status.as_str().and_then(|s| s.parse().ok()));
        if code != Some(200) {
            return Err(code
                .map(|c| ScanFailure::from_http(c as u16))
                .unwrap_or(ScanFailure::Parse));
        }
    } else {
        return Err(ScanFailure::Parse);
    }
    if value
        .pointer("/body/errorMessage")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
    {
        return Err(ScanFailure::Parse);
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFailure {
    Unauthorized,
    Blocked,
    RateLimited,
    Network,
    Service,
    Timeout,
    Parse,
    Storage,
}
impl ScanFailure {
    pub fn from_http(status: u16) -> Self {
        match status {
            401 => Self::Unauthorized,
            403 | 541 => Self::Blocked,
            429 => Self::RateLimited,
            408 => Self::Timeout,
            500 | 502 | 503 | 504 => Self::Service,
            _ => Self::Parse,
        }
    }
    pub fn reason(self) -> &'static str {
        match self {
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Blocked => "BLOCKED",
            Self::RateLimited => "RATE_LIMITED",
            Self::Network => "NETWORK_ERROR",
            Self::Service => "SERVICE_UNAVAILABLE",
            Self::Timeout => "TIMEOUT",
            Self::Parse => "PARSE_ERROR",
            Self::Storage => "STORAGE_ERROR",
        }
    }
    pub fn stops_source(self) -> bool {
        matches!(
            self,
            Self::Unauthorized | Self::Blocked | Self::RateLimited | Self::Parse | Self::Storage
        )
    }
}
