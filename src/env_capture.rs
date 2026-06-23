use serde::{Deserialize, Serialize};
use sysinfo::System;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub os: String,
    pub cpu_brand: String,
    pub physical_cores: Option<usize>,
    pub total_memory_bytes: u64,
}

pub fn capture() -> Environment {
    let mut system = System::new_all();
    system.refresh_all();
    let cpu_brand = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    Environment {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        cpu_brand,
        physical_cores: System::physical_core_count(),
        total_memory_bytes: system.total_memory(),
    }
}
