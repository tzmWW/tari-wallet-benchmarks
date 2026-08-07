use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{Config, ProvenancePolicy};

pub const BUILD_MANIFEST_SCHEMA_VERSION: u32 = 2;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const DEV_SOURCE_BINDINGS: [(&str, &str, &str); 4] = [
    (
        "minotari",
        "minotari_cli",
        "https://github.com/tari-project/minotari-cli.git",
    ),
    (
        "minotari_console_wallet",
        "tari_console_wallet",
        "https://github.com/tari-project/tari.git",
    ),
    (
        "minotari_node",
        "minotari_node",
        "https://github.com/tari-project/tari.git",
    ),
    (
        "minotari_payment_processor",
        "payment_processor",
        "https://github.com/tari-project/minotari_payment_processor.git",
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildManifest {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub sources: BTreeMap<String, SourceProvenance>,
    pub artifacts: BTreeMap<String, BuildArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    pub repository: String,
    pub upstream: UpstreamSource,
    pub patches: Vec<AppliedPatch>,
    pub complete_diff_sha256: String,
    pub result_tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamSource {
    pub revision: String,
    pub commit: String,
    pub tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedPatch {
    pub path: String,
    pub sha256: String,
    pub result_tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildArtifact {
    pub source: String,
    pub source_revision: String,
    pub source_tree: String,
    pub sha256: String,
}

pub(crate) struct ExpectedPatch {
    path: &'static str,
    sha256: &'static str,
    result_tree: &'static str,
}

pub(crate) struct ExpectedSource {
    name: &'static str,
    repository: &'static str,
    upstream_revision: &'static str,
    upstream_commit: &'static str,
    upstream_tree: &'static str,
    patches: &'static [ExpectedPatch],
    complete_diff_sha256: &'static str,
    result_tree: &'static str,
}

pub(crate) struct ExpectedArtifact {
    name: &'static str,
    source: &'static str,
    source_revision: &'static str,
    source_tree: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/build_provenance.rs"));

pub fn verify(config: &Config) -> anyhow::Result<()> {
    let bytes = fs::read(&config.paths.build_manifest).with_context(|| {
        format!(
            "reading build manifest {} (rerun both fetch scripts)",
            config.paths.build_manifest.display()
        )
    })?;
    let manifest: BuildManifest = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "parsing build manifest {}",
            config.paths.build_manifest.display()
        )
    })?;

    let artifact_paths = artifact_paths(config);
    match config.provenance.policy {
        ProvenancePolicy::Canonical => {
            verify_canonical_configured_revisions(config)?;
            verify_canonical_manifest(
                &manifest,
                &artifact_paths,
                Path::new(env!("CARGO_MANIFEST_DIR")),
            )?;
            bail!(
                "this harness build follows the development stack; the canonical policy is retained only for historical schema-v6 profile validation"
            );
        }
        ProvenancePolicy::Local | ProvenancePolicy::Dev => {
            match config.provenance.policy {
                ProvenancePolicy::Local => verify_local_configured_revisions(config, &manifest)?,
                ProvenancePolicy::Dev => verify_dev_configured_revisions(config, &manifest)?,
                ProvenancePolicy::Canonical => unreachable!(),
            }
            if config.provenance.policy == ProvenancePolicy::Dev {
                verify_dev_source_checkouts(
                    config,
                    &manifest,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                )?;
            }
            verify_local_manifest(
                &manifest,
                &artifact_paths,
                Path::new(env!("CARGO_MANIFEST_DIR")),
            )?;
            println!(
                "build manifest PASS: {:?} schema v2 provenance is internally consistent and runtime artifact SHA-256 values match",
                config.provenance.policy
            );
        }
    }
    Ok(())
}

pub fn create_local(
    config: &Config,
    minotari_source: &Path,
    console_wallet_source: &Path,
    node_source: &Path,
    payment_processor_source: &Path,
) -> anyhow::Result<()> {
    if config.provenance.policy != ProvenancePolicy::Local {
        bail!("create-local-manifest requires provenance.policy = \"local\"");
    }
    let source_specs = [
        (
            "minotari_cli",
            minotari_source,
            config.versions.minotari_cli_rev.as_str(),
        ),
        (
            "tari_console_wallet",
            console_wallet_source,
            config.versions.tari_console_wallet_rev.as_str(),
        ),
        (
            "minotari_node",
            node_source,
            config.versions.base_node_rev.as_str(),
        ),
        (
            "payment_processor",
            payment_processor_source,
            config.versions.payment_processor_rev.as_str(),
        ),
    ];
    let sources = source_specs
        .into_iter()
        .map(|(name, checkout, revision)| {
            Ok((
                name.to_string(),
                source_from_clean_checkout(checkout, revision)
                    .with_context(|| format!("reading local source {name}"))?,
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let artifact_specs = [
        (
            "minotari",
            "minotari_cli",
            config.versions.minotari_cli_rev.as_str(),
            config.paths.minotari_binary.as_path(),
        ),
        (
            "minotari_console_wallet",
            "tari_console_wallet",
            config.versions.tari_console_wallet_rev.as_str(),
            config.paths.minotari_console_wallet.as_path(),
        ),
        (
            "minotari_node",
            "minotari_node",
            config.versions.base_node_rev.as_str(),
            config.paths.minotari_node.as_path(),
        ),
        (
            "minotari_payment_processor",
            "payment_processor",
            config.versions.payment_processor_rev.as_str(),
            config.paths.payment_processor_binary.as_path(),
        ),
    ];
    let artifacts = artifact_specs
        .into_iter()
        .map(|(name, source_name, revision, binary)| {
            let source = &sources[source_name];
            Ok((
                name.to_string(),
                BuildArtifact {
                    source: source_name.to_string(),
                    source_revision: revision.to_string(),
                    source_tree: source.result_tree.clone(),
                    sha256: sha256_file(binary)?,
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
    let manifest = BuildManifest {
        schema_version: BUILD_MANIFEST_SCHEMA_VERSION,
        channel: None,
        resolved_at: None,
        sources,
        artifacts,
    };
    verify_local_configured_revisions(config, &manifest)?;
    verify_local_manifest(
        &manifest,
        &artifact_paths(config),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )?;

    if let Some(parent) = config.paths.build_manifest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(&manifest)?;
    bytes.push(b'\n');
    crate::result_profile::durable_atomic_write(&config.paths.build_manifest, &bytes)
        .with_context(|| {
            format!(
                "writing local build manifest {}",
                config.paths.build_manifest.display()
            )
        })?;
    println!(
        "wrote local build manifest {}",
        config.paths.build_manifest.display()
    );
    Ok(())
}

fn source_from_clean_checkout(path: &Path, revision: &str) -> anyhow::Result<SourceProvenance> {
    if !path.join(".git").exists() {
        bail!("{} is not a Git checkout", path.display());
    }
    let status = git_output(path, &["status", "--porcelain", "--untracked-files=all"])?;
    if !status.is_empty() {
        bail!(
            "{} is dirty; commit source changes before creating a local baseline manifest",
            path.display()
        );
    }
    let head = git_output(path, &["rev-parse", "HEAD"])?;
    let selected = git_output(path, &["rev-parse", &format!("{revision}^{{commit}}")])?;
    if head != selected {
        bail!(
            "configured revision {revision} resolves to {selected}, but {} is at {head}",
            path.display()
        );
    }
    let tree = git_output(path, &["rev-parse", "HEAD^{tree}"])?;
    let repository = git_output(path, &["remote", "get-url", "origin"])?;
    Ok(SourceProvenance {
        repository,
        upstream: UpstreamSource {
            revision: revision.to_string(),
            commit: head,
            tree: tree.clone(),
        },
        patches: Vec::new(),
        complete_diff_sha256: EMPTY_SHA256.to_string(),
        result_tree: tree,
    })
}

fn git_output(path: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .with_context(|| format!("running git in {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn artifact_paths(config: &Config) -> BTreeMap<String, PathBuf> {
    BTreeMap::from([
        ("minotari".to_string(), config.paths.minotari_binary.clone()),
        (
            "minotari_console_wallet".to_string(),
            config.paths.minotari_console_wallet.clone(),
        ),
        (
            "minotari_node".to_string(),
            config.paths.minotari_node.clone(),
        ),
        (
            "minotari_payment_processor".to_string(),
            config.paths.payment_processor_binary.clone(),
        ),
    ])
}

fn configured_revisions(config: &Config) -> [(&'static str, &str); 4] {
    [
        ("minotari", config.versions.minotari_cli_rev.as_str()),
        (
            "minotari_console_wallet",
            config.versions.tari_console_wallet_rev.as_str(),
        ),
        ("minotari_node", config.versions.base_node_rev.as_str()),
        (
            "minotari_payment_processor",
            config.versions.payment_processor_rev.as_str(),
        ),
    ]
}

fn verify_canonical_configured_revisions(config: &Config) -> anyhow::Result<()> {
    for (name, revision) in configured_revisions(config) {
        let expected = EXPECTED_ARTIFACTS
            .iter()
            .find(|artifact| artifact.name == name)
            .expect("embedded artifact provenance is complete");
        if revision != expected.source_revision {
            bail!(
                "configured {name} revision {revision} does not match embedded provenance {}",
                expected.source_revision
            );
        }
    }
    Ok(())
}

fn verify_local_configured_revisions(
    config: &Config,
    manifest: &BuildManifest,
) -> anyhow::Result<()> {
    for (name, revision) in configured_revisions(config) {
        let artifact = manifest
            .artifacts
            .get(name)
            .with_context(|| format!("build manifest is missing artifact {name}"))?;
        if revision != artifact.source_revision {
            bail!(
                "configured {name} revision {revision} does not match local build manifest {}",
                artifact.source_revision
            );
        }
    }
    Ok(())
}

fn verify_dev_configured_revisions(
    config: &Config,
    manifest: &BuildManifest,
) -> anyhow::Result<()> {
    if manifest.channel.as_deref() != Some("dev") || manifest.resolved_at.is_none() {
        bail!("dev provenance requires channel=dev and a resolution timestamp");
    }
    let configured = BTreeMap::from(configured_revisions(config));
    for (artifact_name, source_name, repository) in DEV_SOURCE_BINDINGS {
        let requested_revision = configured[artifact_name];
        let artifact = manifest
            .artifacts
            .get(artifact_name)
            .with_context(|| format!("build manifest is missing artifact {artifact_name}"))?;
        if artifact.source != source_name {
            bail!("artifact {artifact_name} must be built from dev source {source_name}");
        }
        let source = manifest.sources.get(&artifact.source).with_context(|| {
            format!(
                "artifact {artifact_name} references missing source {}",
                artifact.source
            )
        })?;
        if source.repository.trim_end_matches(".git") != repository.trim_end_matches(".git") {
            bail!("dev source {source_name} repository is not the expected Tari repository");
        }
        if requested_revision != source.upstream.revision {
            bail!(
                "configured {artifact_name} dev ref {requested_revision} does not match resolved manifest ref {}",
                source.upstream.revision
            );
        }
        if artifact.source_revision != source.upstream.commit {
            bail!(
                "artifact {artifact_name} must record the exact resolved dev commit {}",
                source.upstream.commit
            );
        }
    }
    Ok(())
}

fn verify_dev_source_checkouts(
    config: &Config,
    manifest: &BuildManifest,
    source_root: &Path,
) -> anyhow::Result<()> {
    let source_paths = BTreeMap::from([
        ("minotari_cli", config.paths.cache_dir.join("minotari-cli")),
        ("tari_console_wallet", config.paths.cache_dir.join("tari")),
        ("minotari_node", config.paths.cache_dir.join("tari")),
        (
            "payment_processor",
            config.paths.cache_dir.join("minotari_payment_processor"),
        ),
    ]);
    if manifest.sources.len() != source_paths.len() {
        bail!("dev build manifest source set is not exact");
    }

    for (name, checkout) in source_paths {
        let source = manifest
            .sources
            .get(name)
            .with_context(|| format!("dev build manifest is missing source {name}"))?;
        if !checkout.join(".git").exists() {
            bail!("dev source checkout {} is missing", checkout.display());
        }
        if !git_output(
            &checkout,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty()
        {
            bail!("dev source checkout {} is dirty", checkout.display());
        }
        let repository = git_output(&checkout, &["remote", "get-url", "origin"])?;
        if repository.trim_end_matches(".git") != source.repository.trim_end_matches(".git") {
            bail!("dev source {name} repository does not match its checkout origin");
        }
        if git_output(&checkout, &["rev-parse", "HEAD"])? != source.upstream.commit
            || git_output(&checkout, &["rev-parse", "HEAD^{tree}"])? != source.upstream.tree
        {
            bail!("dev source {name} commit/tree does not match its checkout");
        }
        verify_requested_ref(&checkout, name, source)?;

        let index_dir = tempfile::tempdir()?;
        let index_path = index_dir.path().join("index");
        git_with_index(
            &checkout,
            &index_path,
            &["read-tree", &source.upstream.commit],
        )?;
        for patch in &source.patches {
            let patch_path = source_root.join(&patch.path);
            git_with_index(
                &checkout,
                &index_path,
                &["apply", "--cached", patch_path.to_string_lossy().as_ref()],
            )?;
            let result_tree = git_with_index(&checkout, &index_path, &["write-tree"])?;
            if result_tree != patch.result_tree {
                bail!(
                    "dev source {name} patch {} result tree does not match",
                    patch.path
                );
            }
        }
        let result_tree = git_with_index(&checkout, &index_path, &["write-tree"])?;
        if result_tree != source.result_tree {
            bail!("dev source {name} final result tree does not match");
        }
        let diff = git_bytes_with_index(
            &checkout,
            &index_path,
            &[
                "-c",
                "diff.algorithm=myers",
                "diff",
                "--cached",
                "--full-index",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                &source.upstream.commit,
            ],
        )?;
        if hex::encode(Sha256::digest(diff)) != source.complete_diff_sha256 {
            bail!("dev source {name} complete diff SHA-256 does not match");
        }
    }
    Ok(())
}

fn verify_requested_ref(
    checkout: &Path,
    name: &str,
    source: &SourceProvenance,
) -> anyhow::Result<()> {
    let requested = source.upstream.revision.as_str();
    if requested == "latest-prerelease" {
        let tags = git_output(checkout, &["tag", "--points-at", &source.upstream.commit])?;
        if !tags
            .lines()
            .any(|tag| tag.starts_with('v') && tag.contains("-pre."))
        {
            bail!("dev source {name} commit is not tagged as a prerelease");
        }
        return Ok(());
    }

    let remote_ref = format!("origin/{requested}^{{commit}}");
    if git_output(checkout, &["rev-parse", "--verify", &remote_ref]).is_ok() {
        if !git_succeeds(
            checkout,
            &[
                "merge-base",
                "--is-ancestor",
                &source.upstream.commit,
                remote_ref.trim_end_matches("^{commit}"),
            ],
        )? {
            bail!("dev source {name} commit is not reachable from origin/{requested}");
        }
        return Ok(());
    }

    let tag_ref = format!("refs/tags/{requested}^{{commit}}");
    if git_output(checkout, &["rev-parse", "--verify", &tag_ref])? != source.upstream.commit {
        bail!("dev source {name} tag {requested} does not resolve to the recorded commit");
    }
    Ok(())
}

fn git_succeeds(path: &Path, args: &[&str]) -> anyhow::Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .status()
        .with_context(|| format!("running git in {}", path.display()))?;
    Ok(status.success())
}

fn git_with_index(path: &Path, index: &Path, args: &[&str]) -> anyhow::Result<String> {
    let bytes = git_bytes_with_index(path, index, args)?;
    Ok(String::from_utf8(bytes)?.trim().to_string())
}

fn git_bytes_with_index(path: &Path, index: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .output()
        .with_context(|| format!("running git in {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn verify_canonical_manifest(
    manifest: &BuildManifest,
    artifact_paths: &BTreeMap<String, PathBuf>,
    source_root: &Path,
) -> anyhow::Result<()> {
    if manifest.schema_version != BUILD_MANIFEST_SCHEMA_VERSION {
        bail!(
            "unsupported build manifest schema {}; expected {}",
            manifest.schema_version,
            BUILD_MANIFEST_SCHEMA_VERSION
        );
    }
    if manifest.sources.len() != EXPECTED_SOURCES.len() {
        bail!(
            "build manifest source set is not exact: expected {}, found {}",
            EXPECTED_SOURCES.len(),
            manifest.sources.len()
        );
    }
    for expected in EXPECTED_SOURCES {
        let source = manifest
            .sources
            .get(expected.name)
            .with_context(|| format!("build manifest is missing source {}", expected.name))?;
        verify_source(source_root, expected, source)?;
    }

    if manifest.artifacts.len() != EXPECTED_ARTIFACTS.len()
        || artifact_paths.len() != EXPECTED_ARTIFACTS.len()
    {
        bail!("build manifest artifact set is not exact");
    }
    for expected in EXPECTED_ARTIFACTS {
        let artifact = manifest
            .artifacts
            .get(expected.name)
            .with_context(|| format!("build manifest is missing artifact {}", expected.name))?;
        if artifact.source != expected.source
            || artifact.source_revision != expected.source_revision
            || artifact.source_tree != expected.source_tree
        {
            bail!(
                "build manifest artifact {} source provenance does not match the embedded expectation",
                expected.name
            );
        }
        require_sha256_hex(&artifact.sha256, &format!("artifact {}", expected.name))?;
        let path = artifact_paths
            .get(expected.name)
            .with_context(|| format!("runtime path is missing for artifact {}", expected.name))?;
        if sha256_file(path)? != artifact.sha256 {
            bail!(
                "{} SHA-256 does not match the build manifest",
                expected.name
            );
        }
    }
    Ok(())
}

fn verify_local_manifest(
    manifest: &BuildManifest,
    artifact_paths: &BTreeMap<String, PathBuf>,
    source_root: &Path,
) -> anyhow::Result<()> {
    if manifest.schema_version != BUILD_MANIFEST_SCHEMA_VERSION {
        bail!(
            "unsupported build manifest schema {}; expected {}",
            manifest.schema_version,
            BUILD_MANIFEST_SCHEMA_VERSION
        );
    }
    if manifest.artifacts.len() != artifact_paths.len() {
        bail!("local build manifest artifact set is not exact");
    }

    let mut referenced_sources = std::collections::BTreeSet::new();
    for (name, path) in artifact_paths {
        let artifact = manifest
            .artifacts
            .get(name)
            .with_context(|| format!("build manifest is missing artifact {name}"))?;
        let source = manifest.sources.get(&artifact.source).with_context(|| {
            format!(
                "artifact {name} references missing source {}",
                artifact.source
            )
        })?;
        referenced_sources.insert(artifact.source.as_str());
        if artifact.source_revision != source.upstream.revision
            && artifact.source_revision != source.upstream.commit
        {
            bail!("artifact {name} revision does not match its source ref or resolved commit");
        }
        if artifact.source_tree != source.result_tree {
            bail!("artifact {name} source tree does not match its source result tree");
        }
        require_nonempty(
            &artifact.source_revision,
            &format!("artifact {name} revision"),
        )?;
        require_git_hash(
            &artifact.source_tree,
            &format!("artifact {name} source tree"),
        )?;
        require_sha256_hex(&artifact.sha256, &format!("artifact {name}"))?;
        if sha256_file(path)? != artifact.sha256 {
            bail!("{name} SHA-256 does not match the local build manifest");
        }
    }
    if referenced_sources.len() != manifest.sources.len() {
        bail!("local build manifest contains an unreferenced source");
    }

    for (name, source) in &manifest.sources {
        require_nonempty(&source.repository, &format!("source {name} repository"))?;
        require_nonempty(
            &source.upstream.revision,
            &format!("source {name} upstream revision"),
        )?;
        require_git_hash(
            &source.upstream.commit,
            &format!("source {name} upstream commit"),
        )?;
        require_git_hash(
            &source.upstream.tree,
            &format!("source {name} upstream tree"),
        )?;
        require_git_hash(&source.result_tree, &format!("source {name} result tree"))?;
        require_sha256_hex(
            &source.complete_diff_sha256,
            &format!("source {name} complete diff"),
        )?;
        if source.patches.is_empty() {
            if source.result_tree != source.upstream.tree
                || source.complete_diff_sha256 != EMPTY_SHA256
            {
                bail!("unpatched source {name} must retain its upstream tree and empty diff");
            }
        } else if source
            .patches
            .last()
            .map(|patch| patch.result_tree.as_str())
            != Some(source.result_tree.as_str())
        {
            bail!("source {name} result tree must match its final patch result tree");
        }
        for (index, patch) in source.patches.iter().enumerate() {
            require_sha256_hex(&patch.sha256, &format!("source {name} patch {}", index + 1))?;
            require_git_hash(
                &patch.result_tree,
                &format!("source {name} patch {} result tree", index + 1),
            )?;
            let relative = Path::new(&patch.path);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                bail!("source {name} patch path must stay within the harness checkout");
            }
            if sha256_file(&source_root.join(relative))? != patch.sha256 {
                bail!("source {name} patch {} SHA-256 does not match", patch.path);
            }
        }
    }
    Ok(())
}

fn verify_source(
    source_root: &Path,
    expected: &ExpectedSource,
    source: &SourceProvenance,
) -> anyhow::Result<()> {
    if source.repository != expected.repository
        || source.upstream.revision != expected.upstream_revision
        || source.upstream.commit != expected.upstream_commit
        || source.upstream.tree != expected.upstream_tree
        || source.complete_diff_sha256 != expected.complete_diff_sha256
        || source.result_tree != expected.result_tree
        || source.patches.len() != expected.patches.len()
    {
        bail!(
            "build manifest source {} does not match the embedded upstream/tree provenance",
            expected.name
        );
    }
    for (index, (patch, expected_patch)) in source
        .patches
        .iter()
        .zip(expected.patches.iter())
        .enumerate()
    {
        if patch.path != expected_patch.path
            || patch.sha256 != expected_patch.sha256
            || patch.result_tree != expected_patch.result_tree
        {
            bail!(
                "build manifest source {} patch {} is not the expected ordered patch",
                expected.name,
                index + 1
            );
        }
        let patch_path = source_root.join(expected_patch.path);
        if sha256_file(&patch_path)? != expected_patch.sha256 {
            bail!(
                "tracked patch {} SHA-256 does not match embedded provenance",
                expected_patch.path
            );
        }
    }
    Ok(())
}

fn require_sha256_hex(value: &str, label: &str) -> anyhow::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{label} SHA-256 is not lowercase 64-character hexadecimal");
    }
    Ok(())
}

fn require_git_hash(value: &str, label: &str) -> anyhow::Result<()> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{label} is not lowercase 40-character hexadecimal");
    }
    Ok(())
}

fn require_nonempty(value: &str, label: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        bail!("{label} is empty");
    }
    Ok(())
}

pub(crate) fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("reading {} for SHA-256", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_v2_manifest_verifies_exact_embedded_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let (manifest, paths) = manifest_fixture(dir.path());
        verify_canonical_manifest(&manifest, &paths, Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap();
    }

    #[test]
    fn manifest_rejects_changed_result_tree() {
        let dir = tempfile::tempdir().unwrap();
        let (mut manifest, paths) = manifest_fixture(dir.path());
        manifest
            .sources
            .get_mut("minotari_cli")
            .unwrap()
            .result_tree = "0000000000000000000000000000000000000000".to_string();
        let error =
            verify_canonical_manifest(&manifest, &paths, Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap_err()
                .to_string();
        assert!(error.contains("upstream/tree provenance"));
    }

    #[test]
    fn local_manifest_accepts_a_noncanonical_but_consistent_revision() {
        let dir = tempfile::tempdir().unwrap();
        let (mut manifest, paths) = manifest_fixture(dir.path());
        manifest
            .artifacts
            .get_mut("minotari")
            .unwrap()
            .source_revision = "local-revision".to_string();
        manifest
            .sources
            .get_mut("minotari_cli")
            .unwrap()
            .upstream
            .revision = "local-revision".to_string();

        verify_local_manifest(&manifest, &paths, Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(
            verify_canonical_manifest(&manifest, &paths, Path::new(env!("CARGO_MANIFEST_DIR")))
                .is_err()
        );
    }

    #[test]
    fn dev_manifest_freezes_requested_refs_to_exact_commits() {
        let dir = tempfile::tempdir().unwrap();
        let (mut manifest, paths) = manifest_fixture(dir.path());
        manifest.channel = Some("dev".to_string());
        manifest.resolved_at = Some(chrono::Utc::now());
        let mut config = Config::default();
        config.provenance.policy = ProvenancePolicy::Dev;

        for (artifact_name, requested_ref) in configured_revisions(&config) {
            let artifact = manifest.artifacts.get_mut(artifact_name).unwrap();
            let source = manifest.sources.get_mut(&artifact.source).unwrap();
            source.upstream.revision = requested_ref.to_string();
            artifact.source_revision = source.upstream.commit.clone();
        }
        for (_, source_name, repository) in DEV_SOURCE_BINDINGS {
            manifest.sources.get_mut(source_name).unwrap().repository = repository.to_string();
        }

        verify_dev_configured_revisions(&config, &manifest).unwrap();
        verify_local_manifest(&manifest, &paths, Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();

        manifest.artifacts.get_mut("minotari").unwrap().source = "payment_processor".to_string();
        assert!(verify_dev_configured_revisions(&config, &manifest).is_err());
        manifest.artifacts.get_mut("minotari").unwrap().source = "minotari_cli".to_string();

        manifest
            .artifacts
            .get_mut("minotari")
            .unwrap()
            .source_revision = "0000000000000000000000000000000000000000".to_string();
        assert!(verify_dev_configured_revisions(&config, &manifest).is_err());
    }

    #[test]
    fn manifest_rejects_unknown_fields() {
        let json = r#"{
            "schema_version": 2,
            "sources": {},
            "artifacts": {},
            "untracked_claim": true
        }"#;
        assert!(serde_json::from_str::<BuildManifest>(json).is_err());
    }

    #[test]
    fn local_source_manifest_requires_a_clean_pinned_checkout() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        fs::write(dir.path().join("source.txt"), "source\n").unwrap();
        git(dir.path(), &["add", "source.txt"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=wallet-bench test",
                "-c",
                "user.email=wallet-bench@example.invalid",
                "commit",
                "-m",
                "source",
            ],
        );
        git(
            dir.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/source.git",
            ],
        );
        let revision = git_output(dir.path(), &["rev-parse", "HEAD"]).unwrap();

        let source = source_from_clean_checkout(dir.path(), &revision).unwrap();
        assert_eq!(source.upstream.commit, revision);
        assert_eq!(source.upstream.tree, source.result_tree);
        assert_eq!(source.complete_diff_sha256, EMPTY_SHA256);

        fs::write(dir.path().join("source.txt"), "dirty\n").unwrap();
        assert!(
            source_from_clean_checkout(dir.path(), &revision)
                .unwrap_err()
                .to_string()
                .contains("dirty")
        );
    }

    fn git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn manifest_fixture(root: &Path) -> (BuildManifest, BTreeMap<String, PathBuf>) {
        let sources = EXPECTED_SOURCES
            .iter()
            .map(|source| {
                (
                    source.name.to_string(),
                    SourceProvenance {
                        repository: source.repository.to_string(),
                        upstream: UpstreamSource {
                            revision: source.upstream_revision.to_string(),
                            commit: source.upstream_commit.to_string(),
                            tree: source.upstream_tree.to_string(),
                        },
                        patches: source
                            .patches
                            .iter()
                            .map(|patch| AppliedPatch {
                                path: patch.path.to_string(),
                                sha256: patch.sha256.to_string(),
                                result_tree: patch.result_tree.to_string(),
                            })
                            .collect(),
                        complete_diff_sha256: source.complete_diff_sha256.to_string(),
                        result_tree: source.result_tree.to_string(),
                    },
                )
            })
            .collect();
        let mut artifacts = BTreeMap::new();
        let mut paths = BTreeMap::new();
        for expected in EXPECTED_ARTIFACTS {
            let path = root.join(expected.name);
            fs::write(&path, format!("test artifact {}", expected.name)).unwrap();
            artifacts.insert(
                expected.name.to_string(),
                BuildArtifact {
                    source: expected.source.to_string(),
                    source_revision: expected.source_revision.to_string(),
                    source_tree: expected.source_tree.to_string(),
                    sha256: sha256_file(&path).unwrap(),
                },
            );
            paths.insert(expected.name.to_string(), path);
        }
        (
            BuildManifest {
                schema_version: BUILD_MANIFEST_SCHEMA_VERSION,
                channel: None,
                resolved_at: None,
                sources,
                artifacts,
            },
            paths,
        )
    }
}
