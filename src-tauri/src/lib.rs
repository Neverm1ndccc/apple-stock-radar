mod apple_scan;
mod apple_transport;
mod background_tasks;
mod feishu_config;
mod feishu_notify;
mod tray_refresh;
use radar_core::*;
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{
    Emitter, Manager, State,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

const SERVICE: &str = "local.dingdingguo.stockradar.bark";
const ACCOUNT: &str = "default";
struct AppState {
    db: Mutex<Database>,
    catalog: Catalog,
    directory: PathBuf,
    http: reqwest::Client,
    apple_http: reqwest::Client,
    apple_authorization: Option<radar_core::apple::AppleAuthorization>,
    scanner: Mutex<radar_core::scanner::Scanner>,
    scan_generation: AtomicU64,
    feishu_last_request: tokio::sync::Mutex<Option<Instant>>,
}
fn db(s: &AppState) -> Result<MutexGuard<'_, Database>> {
    s.db.lock()
        .map_err(|_| "本地数据库锁不可用，请重启应用".into())
}
fn read_bark() -> Result<String> {
    let bytes = security_framework::passwords::get_generic_password(SERVICE, ACCOUNT)
        .map_err(|_| "无法从 Keychain 读取 Bark 地址，请重新配置".to_string())?;
    String::from_utf8(bytes).map_err(|_| "Keychain 数据格式错误".into())
}
fn refresh(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        let _ = tray_refresh::with_presentation(&state.db, |presentation| {
            if let Some(tray) = app.tray_by_id("radar") {
                let _ = tray.set_title(presentation.title);
                let _ = tray.set_tooltip(Some(presentation.tooltip));
            }
        });
    }
    let _ = app.emit("radar:changed", ());
}

#[tauri::command]
fn bootstrap(state: State<AppState>) -> Result<Snapshot> {
    let database = db(&state)?;
    let settings = database.settings()?;
    let access = apple_scan::access(&state, &settings);
    let available = access.validate(chrono::Utc::now()).is_ok();
    let runtime = state.scanner.lock().map_err(|_| "扫描状态锁不可用")?;
    let mut scanner = runtime.status.clone();
    if !scanner.running && scanner.stop_reason.is_none() {
        scanner.next_scan_at = runtime.due_at(&settings, access.minimum_interval());
    }
    drop(runtime);
    let at = chrono::Utc::now();
    scanner.budget_capacity = radar_core::request_budget::CAPACITY;
    scanner.budget_refill_seconds = radar_core::request_budget::REFILL_SECONDS;
    scanner.budget_remaining = settings.request_budget(at).remaining(at);
    scanner.budget_ready_at = settings.budget_ready_at().filter(|deadline| *deadline > at);
    scanner.resume_after = settings.resume_after();
    scanner.access_kind = access.kind().into();
    scanner.minimum_interval_seconds =
        available.then_some(settings.scan_minimum_interval(access.minimum_interval()));
    scanner.allowed_locales = access.locales();
    if !available {
        scanner.stop_reason = Some("UNAUTHORIZED".into());
    }
    if settings.paused || !settings.scan_running || settings.mode != Source::Apple || !available {
        scanner.next_scan_at = None;
    }
    let mut targets = database.targets(settings.mode, chrono::Utc::now())?;
    if settings.mode == Source::Apple {
        for target in &mut targets {
            if !available || !scanner.allowed_locales.contains(&target.product.locale) {
                target.state = "UNKNOWN".into();
                target.reason = Some("UNAUTHORIZED".into());
            } else if let Some(reason) = scanner.stop_reason.as_ref() {
                target.state = "UNKNOWN".into();
                target.reason = Some(reason.clone());
            } else if scanner.invalidated_at.is_some_and(|cutoff| {
                target
                    .observed_at
                    .as_ref()
                    .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
                    .is_none_or(|time| time <= cutoff)
            }) {
                target.state = "UNKNOWN".into();
                target.reason = Some("SOURCE_RECHECK".into());
            }
        }
    }
    if available {
        scanner.batch_count = radar_core::apple::batches(
            targets
                .iter()
                .filter(|t| scanner.allowed_locales.contains(&t.product.locale))
                .cloned()
                .collect(),
        )
        .len();
    }
    Ok(Snapshot {
        targets,
        settings,
        deliveries: database.deliveries()?,
        groups: database.groups()?,
        catalog_version: state.catalog.version.clone(),
        automatic_available: available,
        scanner,
    })
}
#[tauri::command]
fn catalog(state: State<AppState>) -> Result<serde_json::Value> {
    serde_json::to_value(&state.catalog).map_err(|e| e.to_string())
}
#[tauri::command]
fn add_targets(
    app: tauri::AppHandle,
    state: State<AppState>,
    product_ids: Vec<String>,
    store_ids: Vec<String>,
    title: String,
) -> Result<AddResult> {
    let result = db(&state)?.add_targets(&state.catalog, &product_ids, &store_ids, &title)?;
    refresh(&app);
    Ok(result)
}
#[tauri::command]
fn update_target(
    app: tauri::AppHandle,
    state: State<AppState>,
    id: String,
    enabled: bool,
    priority: i64,
    archived: bool,
) -> Result<()> {
    state.scan_generation.fetch_add(1, Ordering::SeqCst);
    db(&state)?.update_target(&id, enabled, priority, archived)?;
    refresh(&app);
    Ok(())
}
#[tauri::command]
fn save_settings(app: tauri::AppHandle, state: State<AppState>, settings: Settings) -> Result<()> {
    let database = db(&state)?;
    let previous = database.settings()?;
    let available = apple_scan::available(&state, &settings);
    let settings = settings.user_update(&previous, available)?;
    state.scan_generation.fetch_add(1, Ordering::SeqCst);
    database.save_settings(&settings)?;
    drop(database);
    refresh(&app);
    Ok(())
}
#[tauri::command]
fn batch_update_targets(
    app: tauri::AppHandle,
    state: State<AppState>,
    ids: Vec<String>,
    action: TargetBatchAction,
) -> Result<usize> {
    let count = {
        let mut database = db(&state)?;
        let count = database.batch_update_targets(&ids, action)?;
        if !matches!(
            action,
            TargetBatchAction::EnableFeishu | TargetBatchAction::DisableFeishu
        ) {
            state.scan_generation.fetch_add(1, Ordering::SeqCst);
        }
        count
    };
    refresh(&app);
    Ok(count)
}
#[tauri::command]
fn set_scanning(app: tauri::AppHandle, state: State<AppState>, enabled: bool) -> Result<()> {
    let database = db(&state)?;
    let mut settings = database.settings()?;
    if enabled
        && state
            .scanner
            .lock()
            .map_err(|_| "扫描状态锁不可用")?
            .status
            .running
    {
        return Err("请等待当前扫描完成后再重新开启".into());
    }
    let available = apple_scan::available(&state, &settings);
    settings.set_scanning(enabled, available)?;
    state.scan_generation.fetch_add(1, Ordering::SeqCst);
    database.save_settings(&settings)?;
    drop(database);
    if enabled {
        state
            .scanner
            .lock()
            .map_err(|_| "扫描状态锁不可用")?
            .resume();
    }
    refresh(&app);
    Ok(())
}

#[tauri::command]
async fn record_observation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    input: ObservationInput,
) -> Result<Option<String>> {
    if input.source == Source::Apple {
        return Err("Apple 观测只能由真实来源适配器生成，不能手动录入".into());
    }
    let event = db(&state)?.record(&input, chrono::Utc::now())?;
    dispatch(&app).await;
    refresh(&app);
    Ok(event)
}
#[tauri::command]
async fn scan_now(app: tauri::AppHandle) -> Result<usize> {
    apple_scan::scan(&app, true).await
}
#[tauri::command]
fn simulate(app: tauri::AppHandle, state: State<AppState>) -> Result<usize> {
    let n = db(&state)?.mock_tick()?;
    refresh(&app);
    Ok(n)
}
#[tauri::command]
fn query_history(
    state: State<AppState>,
    filter: radar_core::history::HistoryFilter,
    offset_minutes: i32,
) -> Result<radar_core::history::HistoryResult> {
    let d = db(&state)?;
    d.query_history(d.settings()?.mode, &filter, offset_minutes)
}
#[tauri::command]
fn history(
    state: State<AppState>,
    days: i64,
    target_id: Option<String>,
) -> Result<Vec<HistoryRow>> {
    let d = db(&state)?;
    d.history(d.settings()?.mode, days, target_id.as_deref())
}
#[tauri::command]
fn summary(
    state: State<AppState>,
    days: i64,
    target_id: Option<String>,
    offset_minutes: i32,
) -> Result<Summary> {
    let d = db(&state)?;
    d.summary(
        d.settings()?.mode,
        days,
        target_id.as_deref(),
        offset_minutes,
    )
}
#[tauri::command]
fn set_bark(state: State<AppState>, value: String) -> Result<()> {
    let _ = state;
    let value = bark_url(&value)?.to_string();
    security_framework::passwords::set_generic_password(SERVICE, ACCOUNT, value.as_bytes())
        .map_err(|_| "无法保存到 Keychain".into())
}
#[tauri::command]
fn bark_configured() -> bool {
    read_bark().is_ok()
}
#[tauri::command]
fn remove_bark(app: tauri::AppHandle, state: State<AppState>) -> Result<()> {
    security_framework::passwords::delete_generic_password(SERVICE, ACCOUNT)
        .map_err(|_| "Keychain 删除失败，地址可能已经不存在".to_string())?;
    let database = db(&state)?;
    let mut settings = database.settings()?;
    settings.bark_enabled = false;
    database.save_settings(&settings)?;
    drop(database);
    refresh(&app);
    Ok(())
}
fn local_push(app: &tauri::AppHandle, title: &str, body: &str) -> Result<()> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .sound("default")
        .show()
        .map_err(|_| "系统通知投递失败，请检查系统通知权限".into())
}
async fn bark_push(client: &reqwest::Client, title: &str, body: &str, url: &str) -> Result<()> {
    official_url(url)?;
    let secret = read_bark()?;
    let address = bark_url(&secret)?;
    let key = address.path().trim_matches('/');
    let response=client.post("https://api.day.app/push").json(&serde_json::json!({"device_key":key,"title":title,"body":body,"url":url,"group":"库存雷达","sound":"bell"})).send().await.map_err(|_|"Bark 网络请求失败或超时；凭证已脱敏".to_string())?;
    if !response.status().is_success() {
        return Err(format!("Bark 服务返回 HTTP {}", response.status().as_u16()));
    }
    let response: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "Bark 响应无法解析".to_string())?;
    if response.get("code").and_then(|v| v.as_i64()) != Some(200) {
        return Err("Bark 服务未确认成功，请核对 Key".into());
    }
    Ok(())
}
async fn dispatch(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    feishu_notify::deliver_scan_alert(&state).await;
    let items = match db(&state).and_then(|d| d.pending_deliveries()) {
        Ok(items) => items,
        Err(_) => return,
    };
    for item in items.into_iter().filter(|i| i.status == "PENDING") {
        if item.channel == "feishu" {
            feishu_notify::deliver(&state, &item).await;
            continue;
        }
        let claim = (|| -> Result<bool> {
            let d = db(&state)?;
            let settings = d.settings()?;
            if settings.paused || settings.mode == Source::Mock || settings.mode != item.source {
                return Ok(false);
            }
            d.claim_delivery(&item.id)
        })();
        if claim != Ok(true) {
            continue;
        }
        let settings = match db(&state).and_then(|d| d.settings()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let result = if item.channel == "local" && settings.local_notifications {
            local_push(app, &item.title, &item.body)
        } else if item.channel == "bark" && settings.bark_enabled {
            bark_push(&state.http, &item.title, &item.body, &item.url).await
        } else {
            Err("通知渠道已关闭，未发送".into())
        };
        if let Ok(d) = db(&state) {
            let _ = d.finish_delivery(&item.id, result);
        }
    }
}
#[tauri::command]
async fn test_notification(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    channel: String,
) -> Result<()> {
    let title = "[测试] 库存雷达";
    let body = "通知链路测试成功。这是测试消息，不代表真实库存。";
    match channel.as_str() {
        "local" => local_push(&app, title, body),
        "bark" => bark_push(&state.http, title, body, "https://www.apple.com.cn/shop").await,
        "feishu" => feishu_notify::push(&state, title, body, "https://www.apple.com.cn/shop").await,
        _ => Err("通知渠道无效".into()),
    }
}
#[tauri::command]
fn open_official(app: tauri::AppHandle, url: String) -> Result<()> {
    let url = official_url(&url)?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|_| "无法打开官方页面".into())
}
#[tauri::command]
fn open_main(app: tauri::AppHandle, page: Option<String>) -> Result<()> {
    show_main(&app);
    if let Some(page) = page {
        let _ = app.emit("radar:navigate", page);
    }
    Ok(())
}
fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some(w) = app.get_webview_window("tray") {
        let _ = w.hide();
    }
}

fn data_path(state: &AppState, kind: &str, extension: &str) -> Result<PathBuf> {
    let directory = state.directory.join(kind);
    std::fs::create_dir_all(&directory).map_err(|_| "无法创建导出目录".to_string())?;
    Ok(directory.join(format!(
        "radar-{}.{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ"),
        extension
    )))
}
fn write_atomic(path: &Path, value: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, value).map_err(|_| "无法写入导出文件".to_string())?;
    std::fs::rename(tmp, path).map_err(|_| "无法完成导出".into())
}
#[tauri::command]
fn export_data(state: State<AppState>, format: String) -> Result<String> {
    if !["json", "csv"].contains(&format.as_str()) {
        return Err("格式无效".into());
    }
    let d = db(&state)?;
    let value = d.export(d.settings()?.mode, &format)?;
    let path = data_path(&state, "exports", &format)?;
    write_atomic(&path, value.as_bytes())?;
    Ok(path.display().to_string())
}
#[tauri::command]
fn backup_data(state: State<AppState>) -> Result<String> {
    let path = data_path(&state, "backups", "sqlite")?;
    db(&state)?.backup(&path)?;
    Ok(path.display().to_string())
}
#[tauri::command]
fn list_backups(state: State<AppState>) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(state.directory.join("backups")) else {
        return vec![];
    };
    let mut names: Vec<_> = dir
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|s| s.starts_with("radar-") && s.ends_with(".sqlite"))
        .collect();
    names.sort();
    names.reverse();
    names
}
#[tauri::command]
fn restore_data(app: tauri::AppHandle, state: State<AppState>, name: String) -> Result<()> {
    if name.contains('/')
        || name.contains('\\')
        || !name.starts_with("radar-")
        || !name.ends_with(".sqlite")
    {
        return Err("备份名称无效".into());
    }
    let path = state.directory.join("backups").join(name);
    state.scan_generation.fetch_add(1, Ordering::SeqCst);
    let mut d = db(&state)?;
    d.backup(&data_path(&state, "backups", "sqlite")?)?;
    d.restore(&path)?;
    drop(d);
    refresh(&app);
    Ok(())
}
#[tauri::command]
fn clear_history(app: tauri::AppHandle, state: State<AppState>) -> Result<()> {
    let mut d = db(&state)?;
    let source = d.settings()?.mode;
    state.scan_generation.fetch_add(1, Ordering::SeqCst);
    d.clear(source)?;
    drop(d);
    refresh(&app);
    Ok(())
}
#[tauri::command]
fn prune_history(state: State<AppState>) -> Result<usize> {
    db(&state)?.prune()
}
#[tauri::command]
fn open_data_directory(app: tauri::AppHandle, state: State<AppState>) -> Result<()> {
    app.opener()
        .open_path(state.directory.display().to_string(), None::<&str>)
        .map_err(|_| "无法打开数据目录".into())
}
#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn tray_image() -> tauri::image::Image<'static> {
    let mut rgba = vec![0; 22 * 22 * 4];
    for y in 0..22 {
        for x in 0..22 {
            let dx = x as f32 - 10.5;
            let dy = y as f32 - 10.5;
            let radius = (dx * dx + dy * dy).sqrt();
            if (radius - 8.5).abs() < 0.7
                || (radius - 4.7).abs() < 0.6
                || (dx >= 0.0 && (dy + dx).abs() < 0.8 && radius < 9.0)
                || radius < 1.5
            {
                rgba[(y * 22 + x) * 4 + 3] = 255;
            }
        }
    }
    tauri::image::Image::new_owned(rgba, 22, 22)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent)
                .build(),
        )
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);
            let directory = app.path().app_data_dir()?;
            std::fs::create_dir_all(&directory)?;
            let database =
                Database::open(&directory.join("radar.sqlite")).map_err(std::io::Error::other)?;
            database.prune().map_err(std::io::Error::other)?;
            let catalog = Catalog::load().map_err(std::io::Error::other)?;
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(12))
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let apple_authorization = apple_scan::load_authorization(&directory);
            let apple_http = apple_transport::client()?;
            let mut settings = database.settings().map_err(std::io::Error::other)?;
            if radar_core::apple::AppleAccess::configured(
                settings.apple_personal_access,
                apple_authorization.as_ref(),
            )
            .validate(chrono::Utc::now())
            .is_err()
                && settings.scan_running
            {
                settings.scan_running = false;
                database
                    .save_settings(&settings)
                    .map_err(std::io::Error::other)?;
            }
            let mut scanner = radar_core::scanner::Scanner::default();
            scanner.status.last_finished_at = settings.scan_last_request_at;
            scanner.status.stop_reason = settings.scan_stop_reason;
            scanner.status.invalidated_at = settings.scan_invalidated_at;
            scanner.status.last_http_status = settings.scan_last_http_status;
            scanner.status.consecutive_failures = settings.scan_failure_count;
            app.manage(AppState {
                apple_authorization,
                scanner: Mutex::new(scanner),
                scan_generation: AtomicU64::new(0),
                feishu_last_request: tokio::sync::Mutex::new(None),
                db: Mutex::new(database),
                catalog,
                directory,
                http,
                apple_http,
            });
            let show = MenuItem::with_id(app, "show", "打开库存雷达", true, None::<&str>)?;
            let pause = MenuItem::with_id(app, "pause", "暂停 / 恢复", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &pause, &settings, &quit])?;
            TrayIconBuilder::with_id("radar")
                .icon(tray_image())
                .icon_as_template(true)
                .tooltip("库存雷达 · 手动记录模式")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "settings" => {
                        show_main(app);
                        let _ = app.emit("radar:navigate", "settings");
                    }
                    "pause" => {
                        let s = app.state::<AppState>();
                        if let Ok(d) = db(&s)
                            && let Ok(mut settings) = d.settings()
                        {
                            s.scan_generation.fetch_add(1, Ordering::SeqCst);
                            settings.paused = !settings.paused;
                            let _ = d.save_settings(&settings);
                        }
                        refresh(app);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("tray") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let scale = w.scale_factor().unwrap_or(1.0);
                                let mut x = position.x - 195.0 * scale;
                                let mut y = position.y + 12.0 * scale;
                                if let Ok(Some(monitor)) =
                                    w.monitor_from_point(position.x, position.y)
                                {
                                    let p = monitor.position();
                                    let size = monitor.size();
                                    x = x.clamp(
                                        p.x as f64,
                                        (p.x as f64 + size.width as f64 - 390.0 * scale)
                                            .max(p.x as f64),
                                    );
                                    y = y.min(p.y as f64 + size.height as f64 - 550.0 * scale);
                                }
                                let _ = w
                                    .set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;
            let app_handle = app.handle().clone();
            let notification_handle = app.handle().clone();
            background_tasks::spawn(
                async move {
                    let mut last_tick = Instant::now();
                    let mut last_prune = Instant::now();
                    let mut last_refresh = Instant::now();
                    loop {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        let state = app_handle.state::<AppState>();
                        let settings = db(&state).and_then(|d| d.settings());
                        if let Ok(settings) = settings {
                            if settings.mode == Source::Mock
                                && settings.mock_running
                                && !settings.paused
                            {
                                if last_tick.elapsed().as_secs() >= settings.mock_interval_seconds {
                                    if let Ok(mut d) = db(&state) {
                                        let _ = d.mock_tick();
                                    }
                                    last_tick = Instant::now();
                                    refresh(&app_handle);
                                }
                            } else {
                                last_tick = Instant::now();
                            }
                        }
                        if last_prune.elapsed() > Duration::from_secs(3600) {
                            if let Ok(d) = db(&state) {
                                let _ = d.prune();
                            }
                            last_prune = Instant::now();
                        }
                        if last_refresh.elapsed() >= Duration::from_secs(15) {
                            refresh(&app_handle);
                            last_refresh = Instant::now();
                        }
                        let _ = apple_scan::scan(&app_handle, false).await;
                    }
                },
                async move {
                    loop {
                        dispatch(&notification_handle).await;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                },
            );
            refresh(app.handle());
            Ok(())
        })
        .on_window_event(|w, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = w.hide();
            }
            tauri::WindowEvent::Focused(false) if w.label() == "tray" => {
                let _ = w.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            query_history,
            bootstrap,
            catalog,
            add_targets,
            update_target,
            batch_update_targets,
            save_settings,
            set_scanning,
            record_observation,
            simulate,
            scan_now,
            history,
            summary,
            set_bark,
            feishu_notify::feishu_configured,
            feishu_notify::set_feishu,
            feishu_notify::import_feishu_keychain,
            feishu_notify::remove_feishu,
            feishu_notify::set_feishu_enabled,
            feishu_notify::set_target_feishu,
            feishu_notify::send_target_feishu,
            bark_configured,
            remove_bark,
            test_notification,
            open_official,
            open_main,
            export_data,
            backup_data,
            list_backups,
            restore_data,
            clear_history,
            prune_history,
            open_data_directory,
            quit
        ])
        .build(tauri::generate_context!())
        .expect("应用初始化失败；原数据库保持在本地，请查看终端错误")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main(app);
            }
        });
}
