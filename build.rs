use std::{fs, path::Path, process::Command};

use sha2::{Digest, Sha256};

const PATCHES: &[(&str, &str)] = &[
    (
        "patches/minotari-fixed-range-scan.patch",
        "8efbed4f8cfbd87f5ad83080fd9ad70fdf9b8841b48b13279c9863b38fda807d",
    ),
    (
        "patches/minotari-exact-output-locking.patch",
        "56f65ce897c1f428aeb8858faefeaf691d66e4cfa4e3027bd27b2ac856461b63",
    ),
    (
        "patches/minotari-wallet-password-env.patch",
        "c8f203f78cf5a2549be49e1e52e27474e13955a89c79a54658a0e2c06ae039c9",
    ),
    (
        "patches/payment-processor-fee-rate.patch",
        "69c3001b4474d478822651810dc5f25cae5c8bfede2f9bc756de6ded37dc89fe",
    ),
];

const GENERATED_PROVENANCE: &str = r#"
pub(crate) const EXPECTED_SOURCES: &[ExpectedSource] = &[
    ExpectedSource {
        name: "minotari_cli",
        repository: "https://github.com/tari-project/minotari-cli.git",
        upstream_revision: "360c4848a54d65fd710266233cc9277b0f785e74",
        upstream_commit: "360c4848a54d65fd710266233cc9277b0f785e74",
        upstream_tree: "e9bbd1fb7b538e213e17c2986b85940435adce26",
        patches: &[
            ExpectedPatch {
                path: "patches/minotari-fixed-range-scan.patch",
                sha256: "8efbed4f8cfbd87f5ad83080fd9ad70fdf9b8841b48b13279c9863b38fda807d",
                result_tree: "2fc434e0309f0ee92806eeea97bc33edacfbb793",
            },
            ExpectedPatch {
                path: "patches/minotari-exact-output-locking.patch",
                sha256: "56f65ce897c1f428aeb8858faefeaf691d66e4cfa4e3027bd27b2ac856461b63",
                result_tree: "818201e82cc3ab35cccba2fd1ffa4b95bdc08fd2",
            },
            ExpectedPatch {
                path: "patches/minotari-wallet-password-env.patch",
                sha256: "c8f203f78cf5a2549be49e1e52e27474e13955a89c79a54658a0e2c06ae039c9",
                result_tree: "f36ef55c065732ea9cfcfdfda94f71b7199842e1",
            },
        ],
        complete_diff_sha256: "881428c6a82e1add7a516e16b706c4d168ef14f222085f03cd9b792c523deef7",
        result_tree: "f36ef55c065732ea9cfcfdfda94f71b7199842e1",
    },
    ExpectedSource {
        name: "tari_console_wallet",
        repository: "https://github.com/tari-project/tari.git",
        upstream_revision: "9f5adb7183dc2ec285f5c8fae05f4be9735d9749",
        upstream_commit: "9f5adb7183dc2ec285f5c8fae05f4be9735d9749",
        upstream_tree: "be2020d2eb904507fa20442448ef76b6e8f0d502",
        patches: &[],
        complete_diff_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        result_tree: "be2020d2eb904507fa20442448ef76b6e8f0d502",
    },
    ExpectedSource {
        name: "minotari_node",
        repository: "https://github.com/tari-project/tari.git",
        upstream_revision: "v5.4.0",
        upstream_commit: "03e7ccd3257d669f8d73662bb214602fe0987c17",
        upstream_tree: "cd365137e77901f5ddcc484ef0d2faf3c042c8bf",
        patches: &[],
        complete_diff_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        result_tree: "cd365137e77901f5ddcc484ef0d2faf3c042c8bf",
    },
    ExpectedSource {
        name: "payment_processor",
        repository: "https://github.com/tari-project/minotari_payment_processor.git",
        upstream_revision: "f0572c98cbfac7377412dc6d4094c7d7dfc5de2c",
        upstream_commit: "f0572c98cbfac7377412dc6d4094c7d7dfc5de2c",
        upstream_tree: "add06a544f950f724caa13b972cfc13e5d666c90",
        patches: &[ExpectedPatch {
            path: "patches/payment-processor-fee-rate.patch",
            sha256: "69c3001b4474d478822651810dc5f25cae5c8bfede2f9bc756de6ded37dc89fe",
            result_tree: "8f15669442f3da67fc4636de00b80c666d890c5c",
        }],
        complete_diff_sha256: "8b467bf65003de81ea752092ea3b4f2914e28b284590425d155fda4ad13287d8",
        result_tree: "8f15669442f3da67fc4636de00b80c666d890c5c",
    },
];

pub(crate) const EXPECTED_ARTIFACTS: &[ExpectedArtifact] = &[
    ExpectedArtifact {
        name: "minotari",
        source: "minotari_cli",
        source_revision: "1391dbd2155c96e885379d72b76e33582f0aad87",
        source_tree: "f36ef55c065732ea9cfcfdfda94f71b7199842e1",
    },
    ExpectedArtifact {
        name: "minotari_console_wallet",
        source: "tari_console_wallet",
        source_revision: "9f5adb7183dc2ec285f5c8fae05f4be9735d9749",
        source_tree: "be2020d2eb904507fa20442448ef76b6e8f0d502",
    },
    ExpectedArtifact {
        name: "minotari_node",
        source: "minotari_node",
        source_revision: "v5.4.0",
        source_tree: "cd365137e77901f5ddcc484ef0d2faf3c042c8bf",
    },
    ExpectedArtifact {
        name: "minotari_payment_processor",
        source: "payment_processor",
        source_revision: "f0572c98cbfac7377412dc6d4094c7d7dfc5de2c",
        source_tree: "8f15669442f3da67fc4636de00b80c666d890c5c",
    },
];
"#;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = fs::read_to_string(".git/HEAD")
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }

    for (path, expected) in PATCHES {
        println!("cargo:rerun-if-changed={path}");
        verify_patch_hash(Path::new(path), expected);
    }
    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
    fs::write(
        Path::new(&out_dir).join("build_provenance.rs"),
        GENERATED_PROVENANCE,
    )
    .expect("writing embedded build provenance");

    let commit = std::env::var("GIT_COMMIT").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|commit| commit.trim().to_string())
    });
    println!(
        "cargo:rustc-env=GIT_COMMIT={}",
        commit
            .filter(|commit| !commit.is_empty())
            .as_deref()
            .unwrap_or("unknown")
    );
}

fn verify_patch_hash(path: &Path, expected: &str) {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let actual = hex::encode(Sha256::digest(bytes));
    assert_eq!(
        actual,
        expected,
        "tracked patch {} does not match its immutable expected SHA-256",
        path.display()
    );
}
