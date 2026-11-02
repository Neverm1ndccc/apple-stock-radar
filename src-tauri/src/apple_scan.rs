use super::{AppState, db, refresh};
use chrono::Utc;
use radar_core::{
    Result, Source,
    apple::{self, AppleAccess, AppleAuthorization, ScanFailure},
};
use std::{path::Path, sync::atomic::Ordering, time::Instant};
use tauri::Manager;

/// A permission record must be reviewed and registered separately; the application never invents it.
pub fn load_authorization(directory: &Path) -> Option<AppleAuthorization> {
    let path = directory.join("apple-authorization.json");
    if std::fs::metadata(&path).ok()?.len() > 16384 {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let grant: AppleAuthorization = serde_json::from_slice(&data).ok()?;
    grant.validate(Utc::now()).ok()?;
    Some(grant)
}

pub fn access(state: &AppState, settings: &radar_core::Settings) -> AppleAccess {
    AppleAccess::configured(
        settings.apple_personal_access,
        state.apple_authorization.as_ref(),
    )
}
pub fn available(state: &AppState, settings: &radar_core::Settings) -> bool {
    access(state, settings).validate(Utc::now()).is_ok()
}

async fn fetch(
    state: &AppState,
    batch: &apple::AppleBatch,
    access: &AppleAccess,
    http_status: &mut Option<u16>,
    retry_after: &mut Option<chrono::DateTime<Utc>>,
    http_version: &mut Option<String>,
    watched: &[radar_core::Target],
) -> std::result::Result<Vec<radar_core::ObservationInput>, ScanFailure> {
    let url = apple::accessible_request(access, batch, Utc::now())
        .map_err(|_| ScanFailure::Unauthorized)?;
    let mut response = state
        .apple_http
        .get(url)
        .header(
            reqwest::header::ACCEPT_LANGUAGE,
            if batch.locale == "zh_HK" {
                "zh-HK,zh;q=0.9"
            } else {
                "zh-CN,zh;q=0.9"
            },
        )
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ScanFailure::Timeout
            } else {
                ScanFailure::Network
            }
        })?;
    *http_status = Some(response.status().as_u16());
    *http_version = Some(super::apple_transport::version_name(response.version()).into());
    *retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|value| radar_core::request_budget::retry_after(value, Utc::now()));
    if !response.status().is_success() {
        return Err(ScanFailure::from_http(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > 2_000_000)
    {
        return Err(ScanFailure::Parse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        if e.is_timeout() {
            ScanFailure::Timeout
        } else {
            ScanFailure::Network
        }
    })? {
        if body.len() + chunk.len() > 2_000_000 {
            return Err(ScanFailure::Parse);
        }
        body.extend_from_slice(&chunk);
    }
    batch.parse_coverage(&body, watched)
}

pub async fn scan(app: &tauri::AppHandle, force: bool) -> Result<usize> {
    let state = app.state::<AppState>();
    let generation = state.scan_generation.load(Ordering::SeqCst);
    let mut settings = db(&state)?.settings()?;
    if force {
        settings.scan_running = true;
    }
    if settings.mode != Source::Apple || settings.paused || !settings.scan_running {
        return if force {
            Err("请切换到 Apple 模式并恢复应用后扫描".into())
        } else {
            Ok(0)
        };
    }
    let access = access(&state, &settings);
    if let Err(error) = access.validate(Utc::now()) {
        if settings.scan_running {
            settings.scan_running = false;
            settings.scan_stop_reason = Some("UNAUTHORIZED".into());
            db(&state)?.save_settings(&settings)?;
        }
        let mut scanner = state.scanner.lock().map_err(|_| "扫描状态锁不可用")?;
        scanner.status.stop_reason = Some("UNAUTHORIZED".into());
        scanner.status.next_scan_at = None;
        return Err(error);
    }
    let minimum = access.minimum_interval();
    let locales = access.locales();
    let targets = db(&state)?.targets(Source::Apple, Utc::now())?;
    let batches = apple::batches(
        targets
            .iter()
            .filter(|t| locales.contains(&t.product.locale))
            .cloned()
            .collect(),
    );
    let batch = {
        let mut scanner = state.scanner.lock().map_err(|_| "扫描状态锁不可用")?;
        scanner.status.batch_count = batches.len();
        if batches.is_empty() {
            return if force {
                Err("没有许可地区内的已启用关注目标".into())
            } else {
                Ok(0)
            };
        }
        if !scanner.begin(&settings, minimum, true, Utc::now()) {
            return if force {
                Err("扫描进行中、来源已停用或尚未到允许的下次请求时间".into())
            } else {
                Ok(0)
            };
        }
        let index = apple::next_batch_index(&batches, scanner.cursor).ok_or("没有可扫描批次")?;
        let batch = batches[index].clone();
        scanner.cursor = (index + 1) % batches.len();
        batch
    };
    let reserved = (|| -> Result<bool> {
        let mut d = db(&state)?;
        let current = d.settings()?;
        if generation != state.scan_generation.load(Ordering::SeqCst)
            || current.mode != Source::Apple
            || current.paused
            || (!force && !current.scan_running)
        {
            return Ok(false);
        }
        d.reserve_scan_request(Utc::now(), minimum, force)
    })();
    if reserved != Ok(true) {
        state
            .scanner
            .lock()
            .map_err(|_| "扫描状态锁不可用")?
            .cancel(Utc::now());
        return reserved.map(|_| 0);
    }
    refresh(app);
    let mut http_status = None;
    let mut retry_after = None;
    let mut http_version = None;
    let started = Instant::now();
    let result = fetch(
        &state,
        &batch,
        &access,
        &mut http_status,
        &mut retry_after,
        &mut http_version,
        &targets,
    )
    .await;
    let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let completed_at = Utc::now();
    let failure = result.as_ref().err().copied().or_else(|| {
        result.as_ref().ok().and_then(|inputs| {
            inputs
                .iter()
                .any(|i| i.reason.as_deref() == Some("PARSE_ERROR"))
                .then_some(ScanFailure::Parse)
        })
    });
    let inputs = result.unwrap_or_else(|reason| batch.unknown(reason.reason()));
    let stored = (|| -> Result<usize> {
        let mut d = db(&state)?;
        let mut current = d.settings()?;
        current.apply_scan_result(completed_at, minimum, failure, retry_after);
        current.scan_last_http_status = http_status;
        current.scan_last_http_version = http_version;
        current.scan_last_duration_ms = Some(duration_ms);
        if let Some(failure) = failure {
            current.queue_scan_alert(completed_at, failure, http_status);
        }
        d.save_settings(&current)?;
        if generation != state.scan_generation.load(Ordering::SeqCst)
            || current.mode != Source::Apple
            || current.paused
            || (!force && !current.scan_running && !failure.is_some_and(ScanFailure::stops_source))
        {
            return Ok(0);
        }
        let mut count = 0;
        for input in &inputs {
            d.record(input, completed_at)?;
            count += 1;
        }
        Ok(count)
    })();
    let persisted = db(&state).and_then(|d| d.settings()).ok();
    {
        let mut scanner = state.scanner.lock().map_err(|_| "扫描状态锁不可用")?;
        scanner.status.last_http_status = http_status;
        let failure = failure.or_else(|| stored.is_err().then_some(ScanFailure::Storage));
        if generation != state.scan_generation.load(Ordering::SeqCst)
            && !failure.is_some_and(ScanFailure::stops_source)
        {
            scanner.cancel(completed_at);
        } else {
            scanner.finish(
                &settings,
                minimum,
                completed_at,
                failure,
                stored.as_ref().copied().unwrap_or(0),
            );
        }
        if let Some(settings) = persisted {
            scanner.status.consecutive_failures = settings.scan_failure_count;
            if scanner.status.stop_reason.is_none() {
                scanner.status.next_scan_at = scanner.due_at(&settings, minimum);
            }
        }
    }
    refresh(app);
    stored
}
