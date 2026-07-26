use serde::{Deserialize, Serialize};
use sysinfo::{Disks, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub os: String,
    pub cpu_brand: String,
    pub physical_cores: Option<usize>,
    pub total_memory_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_node_host: Option<String>,
    #[serde(default)]
    pub base_node_network_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_host: Option<String>,
    #[serde(default)]
    pub authority_network_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode1_base_node_service_peer: Option<String>,
}

pub fn capture() -> Environment {
    capture_with_network(None, None, None, None)
}

pub fn capture_for_network(
    base_node_url: &str,
    authority_url: &str,
    mode1_base_node_service_peer: Option<&str>,
) -> Environment {
    capture_for_network_with_data_dir(
        base_node_url,
        authority_url,
        mode1_base_node_service_peer,
        None,
    )
}

pub fn capture_for_network_with_data_dir(
    base_node_url: &str,
    authority_url: &str,
    mode1_base_node_service_peer: Option<&str>,
    data_dir: Option<&std::path::Path>,
) -> Environment {
    capture_with_network(
        Some(base_node_url),
        Some(authority_url),
        mode1_base_node_service_peer,
        data_dir,
    )
}

fn capture_with_network(
    base_node_url: Option<&str>,
    authority_url: Option<&str>,
    mode1_base_node_service_peer: Option<&str>,
    data_dir: Option<&std::path::Path>,
) -> Environment {
    let mut system = System::new_all();
    system.refresh_all();
    let cpu_brand = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let (disk_kind, disk_name) = primary_disk(data_dir);
    let (base_node_host, base_node_path) = base_node_network_path(base_node_url);
    let (authority_host, authority_network_path) = base_node_network_path(authority_url);

    Environment {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        cpu_brand,
        physical_cores: System::physical_core_count(),
        total_memory_bytes: system.total_memory(),
        disk_kind,
        disk_name,
        base_node_host,
        base_node_network_path: base_node_path,
        authority_host,
        authority_network_path,
        mode1_base_node_service_peer: mode1_base_node_service_peer.map(ToString::to_string),
    }
}

fn primary_disk(data_dir: Option<&std::path::Path>) -> (Option<String>, Option<String>) {
    let disks = Disks::new_with_refreshed_list();
    let disk = data_dir
        .and_then(|path| select_mount(path, disks.list().iter().map(|disk| disk.mount_point())))
        .and_then(|mount| disks.list().iter().find(|disk| disk.mount_point() == mount))
        .or_else(|| disks.list().iter().max_by_key(|disk| disk.total_space()));
    let Some(disk) = disk else {
        return (None, None);
    };
    (
        Some(disk.kind().to_string()),
        Some(disk.name().to_string_lossy().to_string()),
    )
}

fn select_mount<'a>(
    data_path: &std::path::Path,
    mounts: impl Iterator<Item = &'a std::path::Path>,
) -> Option<&'a std::path::Path> {
    mounts
        .filter(|mount| mount_matches(data_path, mount))
        .max_by_key(|mount| mount.as_os_str().len())
}

fn mount_matches(data_path: &std::path::Path, mount_point: &std::path::Path) -> bool {
    let data_path = canonicalize_existing_prefix(data_path);
    let mount_point = mount_point
        .canonicalize()
        .unwrap_or_else(|_| mount_point.to_path_buf());
    data_path.starts_with(mount_point)
}

fn canonicalize_existing_prefix(path: &std::path::Path) -> std::path::PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    path.parent()
        .map(canonicalize_existing_prefix)
        .map(|parent| parent.join(path.file_name().unwrap_or_default()))
        .unwrap_or_else(|| path.to_path_buf())
}

fn base_node_network_path(base_node_url: Option<&str>) -> (Option<String>, String) {
    let Some(base_node_url) = base_node_url else {
        return (None, "unknown".to_string());
    };
    let Ok(parsed) = url::Url::parse(base_node_url) else {
        return (None, "unknown".to_string());
    };
    let host = parsed.host_str().map(ToString::to_string);
    let path = match host.as_deref() {
        Some("localhost" | "127.0.0.1" | "::1") => "local",
        Some(_) => "remote",
        None => "unknown",
    };
    (host, path.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn base_node_network_path_classifies_local_and_remote_urls() {
        assert_eq!(
            base_node_network_path(Some("http://127.0.0.1:18142")).1,
            "local"
        );
        assert_eq!(
            base_node_network_path(Some("https://rpc.esmeralda.tari.com")).1,
            "remote"
        );
        assert_eq!(base_node_network_path(Some("not a url")).1, "unknown");
    }

    #[test]
    fn mount_selection_prefers_nested_mounts_and_handles_missing_paths() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("external");
        fs::create_dir_all(nested.join("bench/data")).unwrap();
        assert!(mount_matches(&nested.join("bench/data"), &nested));
        assert!(mount_matches(&nested.join("bench/data"), root.path()));
        assert!(!mount_matches(&root.path().join("other"), &nested));
        assert_eq!(
            select_mount(
                &nested.join("bench/data"),
                [root.path(), nested.as_path()].into_iter(),
            ),
            Some(nested.as_path())
        );
        assert_eq!(
            select_mount(
                &root.path().join("unmatched"),
                [nested.as_path()].into_iter(),
            ),
            None
        );
    }

    #[test]
    fn mount_selection_canonicalizes_symlinked_data_paths() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let link = root.path().join("link");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(mount_matches(&link.join("data"), &target));
    }

    #[test]
    fn mount_selection_handles_external_style_and_unmatched_paths() {
        let root = tempfile::tempdir().unwrap();
        let external = root.path().join("Volumes").join("benchmark-disk");
        fs::create_dir_all(external.join("wallet-bench/.bench-data")).unwrap();
        assert!(mount_matches(
            &external.join("wallet-bench/.bench-data"),
            &external
        ));
        assert!(!mount_matches(
            &root.path().join("other-volume/data"),
            &external
        ));
    }
}
