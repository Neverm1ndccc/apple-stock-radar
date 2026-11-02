use super::{ACCOUNT, AppState, db, refresh};
use radar_core::{
    Result,
    feishu::{self, Credentials},
};
use std::time::{Duration, Instant};
use tauri::{Manager, State};

const SERVICE: &str = "local.dingdingguo.stockradar.feishu";

fn read_legacy_credentials() -> Result<Credentials> {
    let bytes = security_framework::passwords::get_generic_password(SERVICE, ACCOUNT)
        .map_err(|_| "未能读取旧钥匙串配置，可在设置中直接重新填写并保存".to_string())?;
    let credentials: Credentials =
        serde_json::from_slice(&bytes).map_err(|_| "飞书配置无效，请重新保存".to_string())?;
    credentials.validate()?;
    Ok(credentials)
}

fn credentials(state: &AppState) -> Result<Credentials> {
    super::feishu_config::load(&state.directory)
}

async fn wait_for_rate(last: &mut Option<Instant>) {
    if let Some(at) = *last {
        tokio::time::sleep(Duration::from_millis(700).saturating_sub(at.elapsed())).await;
    }
    *last = Some(Instant::now());
}

pub async fn push(state: &AppState, title: &str, body: &str, url: &str) -> Result<()> {
    let mut slot = state.feishu_last_request.lock().await;
    wait_for_rate(&mut slot).await;
    send(state, title, body, url).await
}

pub async fn deliver(state: &AppState, item: &radar_core::Delivery) {
    if credentials(state).is_err() {
        return;
    }
    let mut slot = state.feishu_last_request.lock().await;
    let claimed =
        db(state).and_then(|d| d.claim_feishu_delivery(&item.id, &item.target_id, item.source));
    if claimed != Ok(true) {
        return;
    }
    wait_for_rate(&mut slot).await;
    let result = send(state, &item.title, &item.body, &item.url).await;
    if let Ok(database) = db(state) {
        let _ = database.finish_delivery(&item.id, result);
    }
}

pub async fn deliver_scan_alert(state: &AppState) {
    if credentials(state).is_err() {
        return;
    }
    let mut slot = state.feishu_last_request.lock().await;
    let claim = db(state).and_then(|database| database.claim_scan_alert());
    let Ok(Some(alert)) = claim else {
        return;
    };
    wait_for_rate(&mut slot).await;
    let (title, body) = alert.message();
    let result = send(state, &title, &body, "https://www.apple.com.cn/shop").await;
    if let Ok(database) = db(state) {
        let _ = database.finish_scan_alert(&alert.id, result);
    }
}

async fn send(state: &AppState, title: &str, body: &str, url: &str) -> Result<()> {
    let credentials = credentials(state)?;
    let payload = feishu::payload(
        title,
        body,
        url,
        credentials.secret.as_deref(),
        chrono::Utc::now().timestamp(),
    )?;
    let mut response = state
        .http
        .post(feishu::webhook_url(&credentials.webhook)?)
        .json(&payload)
        .send()
        .await
        .map_err(|_| "飞书网络请求失败或超时；凭证已脱敏".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "飞书返回 HTTP {}，本次未自动重试",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "飞书响应读取失败".to_string())?
    {
        if bytes.len() + chunk.len() > 65536 {
            return Err("飞书响应超出允许大小".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| "飞书响应格式无效".to_string())?;
    feishu::check_response(&value)
}

#[tauri::command]
pub fn feishu_configured(state: State<AppState>) -> bool {
    credentials(&state).is_ok()
}

#[tauri::command]
pub async fn set_feishu(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    webhook: String,
    secret: String,
) -> Result<()> {
    let slot = state.feishu_last_request.lock().await;
    let credentials = Credentials {
        webhook: feishu::webhook_url(&webhook)?.to_string(),
        secret: (!secret.trim().is_empty()).then(|| secret.trim().to_string()),
    };
    credentials.validate()?;
    // Disable and cancel queued messages before changing their destination.
    {
        let database = db(&state)?;
        let mut settings = database.settings()?;
        settings.feishu_enabled = false;
        database.save_settings(&settings)?;
        database.cancel_pending_feishu()?;
    }
    super::feishu_config::save(&state.directory, &credentials)?;
    drop(slot);
    refresh(&app);
    Ok(())
}

/// The only Feishu path allowed to read Keychain; invoked explicitly by the user.
#[tauri::command]
pub async fn import_feishu_keychain(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<()> {
    let slot = state.feishu_last_request.lock().await;
    let credentials = tokio::task::spawn_blocking(read_legacy_credentials)
        .await
        .map_err(|_| "旧飞书配置导入任务失败")??;
    {
        let database = db(&state)?;
        let mut settings = database.settings()?;
        settings.feishu_enabled = false;
        database.save_settings(&settings)?;
        database.cancel_pending_feishu()?;
    }
    super::feishu_config::save(&state.directory, &credentials)?;
    drop(slot);
    refresh(&app);
    Ok(())
}

#[tauri::command]
pub async fn set_feishu_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<()> {
    let slot = state.feishu_last_request.lock().await;
    if enabled {
        credentials(&state)?;
    }
    {
        let database = db(&state)?;
        let mut settings = database.settings()?;
        settings.feishu_enabled = enabled;
        database.save_settings(&settings)?;
        if !enabled {
            database.cancel_pending_feishu()?;
        }
    }
    drop(slot);
    refresh(&app);
    Ok(())
}

#[tauri::command]
pub async fn remove_feishu(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<()> {
    let slot = state.feishu_last_request.lock().await;
    {
        let database = db(&state)?;
        let mut settings = database.settings()?;
        settings.feishu_enabled = false;
        database.save_settings(&settings)?;
        database.cancel_pending_feishu()?;
    }
    super::feishu_config::remove(&state.directory)?;
    drop(slot);
    refresh(&app);
    Ok(())
}

#[tauri::command]
pub fn set_target_feishu(
    app: tauri::AppHandle,
    state: State<AppState>,
    id: String,
    enabled: bool,
) -> Result<()> {
    db(&state)?.set_target_feishu(&id, enabled)?;
    refresh(&app);
    Ok(())
}

#[tauri::command]
pub async fn send_target_feishu(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<()> {
    let app_state = app.state::<AppState>();
    let mut slot = app_state.feishu_last_request.lock().await;
    // Reuse Rust's effective source/expiry status, not the frontend's display text.
    let snapshot = super::bootstrap(state)?;
    if snapshot.settings.paused || !snapshot.settings.feishu_enabled {
        return Err("请恢复应用并在设置中开启飞书通知".into());
    }
    let target = snapshot
        .targets
        .iter()
        .find(|target| target.id == id)
        .ok_or("关注不存在")?;
    if !target.enabled || !target.feishu_enabled {
        return Err("请先启用该关注及其飞书通知开关".into());
    }
    let (title, body, url) = feishu::target_message(target)?;
    wait_for_rate(&mut slot).await;
    send(&app_state, &title, &body, &url).await
}
