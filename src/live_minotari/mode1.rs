use std::time::Duration;

use tokio::time;

pub(super) const MODE1_DB_LOCK_RETRY_ATTEMPTS: u32 = 8;

pub(super) fn mode1_status_is_database_locked(status: &tonic::Status) -> bool {
    let message = status.message().to_ascii_lowercase();
    message.contains("database is locked")
}

pub(super) async fn wait_after_mode1_database_lock(label: &str, retry: u32) {
    let backoff = Duration::from_millis(250 * u64::from(retry));
    println!(
        "{label} hit wallet database lock; retry {retry}/{MODE1_DB_LOCK_RETRY_ATTEMPTS} after {}ms",
        backoff.as_millis()
    );
    wait_one_interval(backoff).await;
}

async fn wait_one_interval(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let mut interval = time::interval(duration);
    interval.tick().await;
    interval.tick().await;
}
