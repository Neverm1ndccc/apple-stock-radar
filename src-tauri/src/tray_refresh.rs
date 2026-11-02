use radar_core::{Database, Result, Source};
use std::sync::Mutex;

pub struct Presentation {
    pub title: Option<String>,
    pub tooltip: &'static str,
}

/// Native tray setters synchronously wait for the main thread. Release the
/// database before entering that boundary: main-thread IPC may need this lock.
pub fn with_presentation(
    database: &Mutex<Database>,
    apply: impl FnOnce(Presentation),
) -> Result<()> {
    let presentation = {
        let database = database.lock().map_err(|_| "本地数据库锁不可用")?;
        let settings = database.settings()?;
        let count = database
            .targets(settings.mode, chrono::Utc::now())?
            .iter()
            .filter(|target| target.enabled && target.state == "IN_STOCK")
            .count();
        Presentation {
            title: if settings.paused {
                Some("Ⅱ".into())
            } else if count > 0 {
                Some(count.to_string())
            } else {
                None
            },
            tooltip: if settings.paused {
                "库存雷达 · 已暂停"
            } else if settings.mode == Source::Apple {
                "库存雷达 · Apple 来源扫描"
            } else if settings.mode == Source::Mock {
                "库存雷达 · 模拟演示，无真实库存查询"
            } else {
                "库存雷达 · 手动记录模式"
            },
        }
    };
    apply(presentation);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };

    #[test]
    fn main_thread_can_read_database_while_background_refresh_waits_for_ui() {
        let database = Arc::new(Mutex::new(Database::memory().unwrap()));
        let worker_database = Arc::clone(&database);
        let (ui_requested, ui_requests) = mpsc::channel();
        let (ui_finished, ui_response) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            with_presentation(&worker_database, |_| {
                ui_requested.send(()).unwrap();
                ui_response.recv_timeout(Duration::from_secs(2)).unwrap();
            })
            .unwrap();
        });
        // Model a native tray call waiting on the UI while bootstrap runs there.
        ui_requests.recv_timeout(Duration::from_secs(2)).unwrap();
        {
            let database = database
                .try_lock()
                .expect("tray UI dispatch must not retain the database lock");
            database.settings().unwrap();
        }
        ui_finished.send(()).unwrap();
        worker.join().unwrap();
    }
}
