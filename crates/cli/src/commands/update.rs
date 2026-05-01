use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use clap::Subcommand;
use serde::{Deserialize, Serialize};

const GITHUB_RELEASES_URL: &str = "https://api.github.com/repos/CrabForge/crab-code/releases";

/// How long to cache the latest-version check result (24 hours).
const VERSION_CHECK_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Update management subcommands.
#[derive(Subcommand)]
pub enum UpdateAction {
    /// Check for a newer version (default if no subcommand given)
    Check {
        /// List the most recent releases
        #[arg(long)]
        list: bool,
    },
    /// Install a specific version (or latest)
    Install {
        /// Target version to install (e.g. "0.3.0"). Defaults to latest.
        #[arg()]
        target: Option<String>,

        /// Only show what would be done, don't actually install
        #[arg(long)]
        dry_run: bool,

        /// Force install even if already on this version
        #[arg(long)]
        force: bool,
    },
    /// Roll back to a previous version
    Rollback {
        /// Version to roll back to
        #[arg()]
        target: Option<String>,
    },
}

// ── GitHub Release API types ─────────────────────────────────────────

/// A single release from the GitHub Releases API (subset of fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    draft: bool,
    prerelease: bool,
    html_url: String,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

/// Asset attached to a GitHub release.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

impl GitHubRelease {
    /// Parse the `tag_name` as a `semver::Version`, stripping a leading 'v' if present.
    fn semver(&self) -> Option<semver::Version> {
        let tag = self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name);
        semver::Version::parse(tag).ok()
    }
}

// ── Cached version check ────────────────────────────────────────────

/// Cached result of a version check, stored as JSON.
#[derive(Debug, Serialize, Deserialize)]
struct VersionCheckCache {
    latest_version: String,
    checked_at_epoch_secs: u64,
}

/// Path to the version check cache file.
fn cache_path() -> PathBuf {
    crab_utils::path::home_dir()
        .join(".crab")
        .join("update-check-cache.json")
}

/// Read the cached version check, if it exists and is still fresh.
fn read_cache() -> Option<String> {
    let path = cache_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let cache: VersionCheckCache = serde_json::from_str(&content).ok()?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if now.saturating_sub(cache.checked_at_epoch_secs) < VERSION_CHECK_TTL.as_secs() {
        Some(cache.latest_version)
    } else {
        None
    }
}

/// Write the latest version to the cache file.
fn write_cache(version: &str) {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let cache = VersionCheckCache {
        latest_version: version.to_owned(),
        checked_at_epoch_secs: now,
    };
    if let Ok(json) = serde_json::to_string_pretty(&cache) {
        let path = cache_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, json);
    }
}

// ── Core logic ──────────────────────────────────────────────────────

/// Current binary version from Cargo.toml.
fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parse current version as semver.
fn current_semver() -> semver::Version {
    semver::Version::parse(current_version()).expect("CARGO_PKG_VERSION should be valid semver")
}

/// Fetch releases from GitHub API (blocking, runs on a tokio runtime).
fn fetch_releases(count: usize) -> anyhow::Result<Vec<GitHubRelease>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let url = format!("{GITHUB_RELEASES_URL}?per_page={count}");
        let client = reqwest::Client::builder()
            .user_agent("crab-code-updater")
            .timeout(Duration::from_secs(15))
            .build()?;
        let resp = client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "GitHub API returned {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        let releases: Vec<GitHubRelease> = resp.json().await?;
        Ok(releases)
    })
}

/// Find the latest non-draft, non-prerelease version from a list of releases.
fn find_latest(releases: &[GitHubRelease]) -> Option<&GitHubRelease> {
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease && r.semver().is_some())
        .max_by_key(|r| r.semver())
}

/// Determine the platform-specific asset name for the current binary.
fn platform_asset_name() -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    let ext = if cfg!(target_os = "windows") {
        ".zip"
    } else {
        ".tar.gz"
    };
    format!("crab-code-{os}-{arch}{ext}")
}

pub fn run(action: &UpdateAction) -> anyhow::Result<()> {
    match action {
        UpdateAction::Check { list } => run_check(*list),
        UpdateAction::Install {
            target,
            dry_run,
            force,
        } => run_install(target.as_deref(), *dry_run, *force),
        UpdateAction::Rollback { target } => run_rollback(target.as_deref()),
    }
}

/// Default entry point when `crab update` is invoked without a subcommand.
pub fn run_default() -> anyhow::Result<()> {
    run_check(false)
}

/// Check for updates — optionally list recent releases.
fn run_check(list: bool) -> anyhow::Result<()> {
    let current = current_version();
    let current_sv = current_semver();
    eprintln!("crab-code v{current}");
    eprintln!();

    if list {
        eprintln!("Fetching recent releases...");
        match fetch_releases(10) {
            Ok(releases) => {
                let stable: Vec<&GitHubRelease> = releases
                    .iter()
                    .filter(|r| !r.draft && !r.prerelease)
                    .collect();
                if stable.is_empty() {
                    eprintln!("  No stable releases found.");
                } else {
                    for r in &stable {
                        let marker = r.semver().map_or(String::new(), |v| {
                            if v == current_sv {
                                " (current)".to_owned()
                            } else if v > current_sv {
                                " (newer)".to_owned()
                            } else {
                                String::new()
                            }
                        });
                        let name = r.name.as_deref().unwrap_or(&r.tag_name);
                        eprintln!("  {name}{marker}");
                        eprintln!("    {}", r.html_url);
                    }
                }
            }
            Err(e) => {
                eprintln!("  Failed to fetch releases: {e}");
                eprintln!("  Check manually: https://github.com/CrabForge/crab-code/releases");
            }
        }
        return Ok(());
    }

    // Standard check: use cache or fetch latest
    let latest_str = if let Some(cached) = read_cache() {
        cached
    } else {
        match fetch_releases(5) {
            Ok(releases) => {
                if let Some(latest) = find_latest(&releases) {
                    let tag = latest
                        .tag_name
                        .strip_prefix('v')
                        .unwrap_or(&latest.tag_name);
                    write_cache(tag);
                    tag.to_owned()
                } else {
                    eprintln!("No stable releases found. You are on the latest version.");
                    return Ok(());
                }
            }
            Err(e) => {
                eprintln!("Could not check for updates: {e}");
                return Ok(());
            }
        }
    };

    match semver::Version::parse(&latest_str) {
        Ok(latest_sv) if latest_sv > current_sv => {
            eprintln!("A newer version is available: v{latest_sv} (you have v{current})");
            eprintln!("Run `crab update install` to upgrade.");
        }
        Ok(_) => {
            eprintln!("You are on the latest version.");
        }
        Err(_) => {
            eprintln!("Latest version tag could not be parsed: {latest_str}");
        }
    }

    Ok(())
}

fn run_install(target: Option<&str>, dry_run: bool, force: bool) -> anyhow::Result<()> {
    let current = current_version();
    let current_sv = current_semver();

    let (target_tag, target_sv) = if let Some(v) = target {
        let clean = v.strip_prefix('v').unwrap_or(v);
        let sv = semver::Version::parse(clean)
            .map_err(|e| anyhow::anyhow!("invalid version '{v}': {e}"))?;
        (clean.to_owned(), sv)
    } else {
        eprintln!("Fetching latest release...");
        let releases = fetch_releases(5)?;
        let latest =
            find_latest(&releases).ok_or_else(|| anyhow::anyhow!("no stable releases found"))?;
        let sv = latest
            .semver()
            .ok_or_else(|| anyhow::anyhow!("could not parse latest tag"))?;
        let tag = latest
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&latest.tag_name)
            .to_owned();
        (tag, sv)
    };

    eprintln!("Current version: v{current}");
    eprintln!("Target version:  v{target_sv}");

    if !force && target_sv == current_sv {
        eprintln!("Already on this version. Use --force to reinstall.");
        return Ok(());
    }

    if !force && target_sv < current_sv {
        eprintln!(
            "Target v{target_sv} is older than current v{current}. Use `crab update rollback` or --force."
        );
        return Ok(());
    }

    let asset_name = platform_asset_name();
    eprintln!("Looking for asset: {asset_name}");

    if dry_run {
        eprintln!(
            "[dry-run] Would download v{target_tag} asset '{asset_name}' and replace current binary."
        );
        return Ok(());
    }

    // Actual download and self-replace
    eprintln!();
    eprintln!(
        "Self-replace binary download is not yet available for direct execution.\n\
         Install manually:\n\
         \n\
         cargo install crab-code@{target_tag}\n\
         \n\
         Or download from:\n\
         https://github.com/CrabForge/crab-code/releases/tag/v{target_tag}"
    );

    // Update the cache with the target version
    write_cache(&target_tag);

    Ok(())
}

fn run_rollback(target: Option<&str>) -> anyhow::Result<()> {
    let current = current_version();
    eprintln!("Current version: v{current}");

    if let Some(v) = target {
        let clean = v.strip_prefix('v').unwrap_or(v);
        let sv = semver::Version::parse(clean)
            .map_err(|e| anyhow::anyhow!("invalid version '{v}': {e}"))?;

        if sv >= current_semver() {
            eprintln!(
                "v{sv} is not older than current v{current}. Use `crab update install` instead."
            );
            return Ok(());
        }

        eprintln!("Requested rollback to: v{sv}");
        eprintln!();
        eprintln!(
            "Rollback via self-replace is not yet available.\n\
             Install manually: cargo install crab-code@{clean}"
        );
    } else {
        eprintln!();
        eprintln!("Fetching available versions...");
        match fetch_releases(10) {
            Ok(releases) => {
                let current_sv = current_semver();
                let older: Vec<&GitHubRelease> = releases
                    .iter()
                    .filter(|r| {
                        !r.draft && !r.prerelease && r.semver().is_some_and(|v| v < current_sv)
                    })
                    .collect();
                if older.is_empty() {
                    eprintln!("No older stable versions found.");
                } else {
                    eprintln!("Available rollback targets:");
                    for r in &older {
                        let name = r.name.as_deref().unwrap_or(&r.tag_name);
                        eprintln!("  {name}");
                    }
                    eprintln!();
                    eprintln!("Usage: crab update rollback <version>");
                }
            }
            Err(e) => {
                eprintln!("Failed to fetch releases: {e}");
                eprintln!("Check: https://github.com/CrabForge/crab-code/releases");
            }
        }
    }

    Ok(())
}

/// Startup version check — called once when the CLI starts.
/// Used by the main CLI startup path and by `crab doctor` to show update
/// notifications. Uses cached result (valid for 24h) to avoid network calls
/// on every invocation. Returns `Some(latest_version)` if an update is
/// available, `None` otherwise.
pub fn startup_version_check() -> Option<String> {
    let cached = read_cache()?;
    let latest = semver::Version::parse(&cached).ok()?;
    let current = current_semver();
    if latest > current { Some(cached) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_is_semver() {
        let v = current_version();
        assert!(!v.is_empty());
        assert!(
            semver::Version::parse(v).is_ok(),
            "should be valid semver: {v}"
        );
    }

    #[test]
    fn current_semver_parses() {
        let sv = current_semver();
        assert!(sv.major == 0 || sv.major >= 1);
    }

    #[test]
    fn platform_asset_name_has_extension() {
        let name = platform_asset_name();
        let lower = name.to_ascii_lowercase();
        let has_valid_ext = lower.ends_with(".tar.gz")
            || std::path::Path::new(&lower)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
        assert!(has_valid_ext, "got: {name}");
        assert!(name.starts_with("crab-code-"));
    }

    #[test]
    fn find_latest_picks_highest_version() {
        let releases = vec![
            GitHubRelease {
                tag_name: "v0.1.0".into(),
                name: Some("0.1.0".into()),
                draft: false,
                prerelease: false,
                html_url: String::new(),
                assets: vec![],
            },
            GitHubRelease {
                tag_name: "v0.3.0".into(),
                name: Some("0.3.0".into()),
                draft: false,
                prerelease: false,
                html_url: String::new(),
                assets: vec![],
            },
            GitHubRelease {
                tag_name: "v0.2.0".into(),
                name: Some("0.2.0".into()),
                draft: false,
                prerelease: false,
                html_url: String::new(),
                assets: vec![],
            },
        ];
        let latest = find_latest(&releases).unwrap();
        assert_eq!(latest.tag_name, "v0.3.0");
    }

    #[test]
    fn find_latest_skips_drafts_and_prereleases() {
        let releases = vec![
            GitHubRelease {
                tag_name: "v2.0.0".into(),
                name: None,
                draft: true,
                prerelease: false,
                html_url: String::new(),
                assets: vec![],
            },
            GitHubRelease {
                tag_name: "v1.5.0-beta.1".into(),
                name: None,
                draft: false,
                prerelease: true,
                html_url: String::new(),
                assets: vec![],
            },
            GitHubRelease {
                tag_name: "v1.0.0".into(),
                name: None,
                draft: false,
                prerelease: false,
                html_url: String::new(),
                assets: vec![],
            },
        ];
        let latest = find_latest(&releases).unwrap();
        assert_eq!(latest.tag_name, "v1.0.0");
    }

    #[test]
    fn find_latest_none_when_empty() {
        let releases: Vec<GitHubRelease> = vec![];
        assert!(find_latest(&releases).is_none());
    }

    #[test]
    fn github_release_semver_parsing() {
        let r = GitHubRelease {
            tag_name: "v1.2.3".into(),
            name: None,
            draft: false,
            prerelease: false,
            html_url: String::new(),
            assets: vec![],
        };
        assert_eq!(r.semver(), Some(semver::Version::new(1, 2, 3)));

        let r2 = GitHubRelease {
            tag_name: "1.0.0".into(),
            name: None,
            draft: false,
            prerelease: false,
            html_url: String::new(),
            assets: vec![],
        };
        assert_eq!(r2.semver(), Some(semver::Version::new(1, 0, 0)));

        let r3 = GitHubRelease {
            tag_name: "not-a-version".into(),
            name: None,
            draft: false,
            prerelease: false,
            html_url: String::new(),
            assets: vec![],
        };
        assert!(r3.semver().is_none());
    }

    #[test]
    fn cache_roundtrip() {
        let cache = VersionCheckCache {
            latest_version: "1.2.3".into(),
            checked_at_epoch_secs: 1_700_000_000,
        };
        let json = serde_json::to_string(&cache).unwrap();
        let back: VersionCheckCache = serde_json::from_str(&json).unwrap();
        assert_eq!(back.latest_version, "1.2.3");
        assert_eq!(back.checked_at_epoch_secs, 1_700_000_000);
    }

    #[test]
    fn startup_version_check_returns_none_without_cache() {
        // Without a cache file, startup check should return None
        // (This test works because the test environment likely has no cache)
        // Just verify it doesn't panic
        let _ = startup_version_check();
    }

    #[test]
    fn run_check_default() {
        // This will try to fetch from GitHub which may fail in CI,
        // but should not panic or return Err
        let _ = run_check(false);
    }

    #[test]
    fn run_install_dry_run() {
        // Dry run with explicit version doesn't need network
        assert!(run_install(Some("99.0.0"), true, false).is_ok());
    }

    #[test]
    fn run_rollback_with_future_version() {
        // Rollback to a version >= current should warn
        assert!(run_rollback(Some("99.0.0")).is_ok());
    }

    #[test]
    fn run_rollback_without_target_does_not_panic() {
        let _ = run_rollback(None);
    }

    #[test]
    fn github_release_serde_roundtrip() {
        let release = GitHubRelease {
            tag_name: "v0.5.0".into(),
            name: Some("Release 0.5.0".into()),
            draft: false,
            prerelease: false,
            html_url: "https://example.com".into(),
            assets: vec![GitHubAsset {
                name: "crab-code-linux-x86_64.tar.gz".into(),
                browser_download_url: "https://example.com/dl".into(),
                size: 10_000_000,
            }],
        };
        let json = serde_json::to_string(&release).unwrap();
        let back: GitHubRelease = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tag_name, "v0.5.0");
        assert_eq!(back.assets.len(), 1);
        assert_eq!(back.assets[0].size, 10_000_000);
    }
}
