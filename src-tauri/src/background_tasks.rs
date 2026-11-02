use std::future::Future;

/// A notification may wait for a system credential prompt indefinitely. It must
/// never be awaited by the inventory scheduler or hold one of its locks.
pub fn spawn(
    scans: impl Future<Output = ()> + Send + 'static,
    notifications: impl Future<Output = ()> + Send + 'static,
) {
    tauri::async_runtime::spawn(scans);
    tauri::async_runtime::spawn(notifications);
}

#[cfg(test)]
mod tests {
    #[test]
    fn waiting_for_notification_credentials_does_not_stop_inventory_ticks() {
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        let (scan_tx, scan_rx) = std::sync::mpsc::channel();
        let (notice_tx, notice_rx) = std::sync::mpsc::channel();
        super::spawn(
            async move {
                for tick in 0..3 {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    scan_tx.send(tick).unwrap();
                }
            },
            async move {
                notice_tx.send(()).unwrap();
                let _ = waiting.await;
            },
        );
        notice_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        for tick in 0..3 {
            assert_eq!(
                scan_rx
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap(),
                tick
            );
        }
        release.send(()).unwrap();
    }
}
