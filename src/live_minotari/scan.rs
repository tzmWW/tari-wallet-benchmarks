use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::time;

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ResourcePeaks {
    pub(super) peak_rss_bytes: Option<u64>,
    pub(super) peak_cpu_percent: Option<f32>,
}

pub(super) async fn with_resource_sampling<F, T>(pid: Option<u32>, future: F) -> (T, ResourcePeaks)
where
    F: Future<Output = T>,
{
    let Some(pid) = pid else {
        return (future.await, ResourcePeaks::default());
    };
    let running = Arc::new(AtomicBool::new(true));
    let sampler = tokio::spawn(sample_process_resources(pid, running.clone()));
    let output = future.await;
    running.store(false, Ordering::Relaxed);
    let peaks = sampler.await.unwrap_or_default();
    (output, peaks)
}

async fn sample_process_resources(pid: u32, running: Arc<AtomicBool>) -> ResourcePeaks {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    let mut peaks = ResourcePeaks::default();
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        if let Some(process) = system.process(pid) {
            peaks.peak_rss_bytes = Some(
                peaks
                    .peak_rss_bytes
                    .unwrap_or_default()
                    .max(process.memory()),
            );
            peaks.peak_cpu_percent = Some(
                peaks
                    .peak_cpu_percent
                    .unwrap_or_default()
                    .max(process.cpu_usage()),
            );
        }
        if !running.load(Ordering::Relaxed) {
            break;
        }
    }
    peaks
}
