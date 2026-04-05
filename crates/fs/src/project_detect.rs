//! Project type detection, `.gitignore` generation, and dependency scanning.
//!
//! Provides [`ProjectDetector`] for identifying project types from marker files,
//! [`GitignoreGenerator`] for producing language-appropriate `.gitignore` content,
//! and [`DependencyScanner`] for extracting dependency lists from manifest files.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ── Project type ─────────────────────────────────────────────────────

/// Recognized project types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectType {
    Rust,
    Node,
    Python,
    Go,
    Java,
    Cpp,
    Ruby,
    Swift,
}

impl std::fmt::Display for ProjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "Rust"),
            Self::Node => write!(f, "Node.js"),
            Self::Python => write!(f, "Python"),
            Self::Go => write!(f, "Go"),
            Self::Java => write!(f, "Java"),
            Self::Cpp => write!(f, "C/C++"),
            Self::Ruby => write!(f, "Ruby"),
            Self::Swift => write!(f, "Swift"),
        }
    }
}

/// Marker files that identify each project type.
const PROJECT_MARKERS: &[(ProjectType, &[&str])] = &[
    (ProjectType::Rust, &["Cargo.toml"]),
    (ProjectType::Node, &["package.json"]),
    (
        ProjectType::Python,
        &["requirements.txt", "pyproject.toml", "setup.py", "Pipfile"],
    ),
    (ProjectType::Go, &["go.mod"]),
    (
        ProjectType::Java,
        &["pom.xml", "build.gradle", "build.gradle.kts"],
    ),
    (
        ProjectType::Cpp,
        &["CMakeLists.txt", "Makefile", "meson.build"],
    ),
    (ProjectType::Ruby, &["Gemfile"]),
    (ProjectType::Swift, &["Package.swift", "*.xcodeproj"]),
];

// ── ProjectDetector ──────────────────────────────────────────────────

/// Result of project detection.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectInfo {
    /// Detected project types (a project can be multi-language).
    pub types: Vec<ProjectType>,
    /// Whether this appears to be a monorepo (multiple sub-projects).
    pub is_monorepo: bool,
    /// Detected sub-project directories (for monorepos).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sub_projects: Vec<String>,
}

/// Detect project types and structure from a directory.
///
/// Scans the root directory for marker files and checks for monorepo patterns.
///
/// # Errors
///
/// Returns an error if the directory cannot be read.
pub fn detect_project(root: &Path) -> crab_common::Result<ProjectInfo> {
    let mut types = Vec::new();
    let mut sub_projects = Vec::new();

    // Check root-level markers
    for (project_type, markers) in PROJECT_MARKERS {
        for marker in *markers {
            if marker.contains('*') {
                // Glob pattern — check if any matching entry exists
                if has_glob_match(root, marker) {
                    if !types.contains(project_type) {
                        types.push(*project_type);
                    }
                    break;
                }
            } else if root.join(marker).exists() {
                if !types.contains(project_type) {
                    types.push(*project_type);
                }
                break;
            }
        }
    }

    // Detect monorepo by checking immediate subdirectories for their own markers
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                let name = entry.file_name();
                let dir_name = name.to_string_lossy();
                // Skip hidden dirs and common non-project dirs
                if dir_name.starts_with('.')
                    || dir_name == "node_modules"
                    || dir_name == "target"
                    || dir_name == "vendor"
                    || dir_name == "dist"
                    || dir_name == "build"
                {
                    continue;
                }
                let sub_path = entry.path();
                for (_pt, markers) in PROJECT_MARKERS {
                    let has_marker = markers.iter().any(|m| {
                        if m.contains('*') {
                            has_glob_match(&sub_path, m)
                        } else {
                            sub_path.join(m).exists()
                        }
                    });
                    if has_marker {
                        sub_projects.push(dir_name.to_string());
                        break;
                    }
                }
            }
        }
    }

    sub_projects.sort();
    let is_monorepo = sub_projects.len() >= 2;

    types.sort();
    Ok(ProjectInfo {
        types,
        is_monorepo,
        sub_projects,
    })
}

/// Check if root contains any file matching a simple glob like `*.xcodeproj`.
fn has_glob_match(root: &Path, pattern: &str) -> bool {
    let suffix = pattern.strip_prefix('*').unwrap_or(pattern);
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(suffix) {
                return true;
            }
        }
    }
    false
}

// ── GitignoreGenerator ───────────────────────────────────────────────

/// Generate `.gitignore` content for a project type.
#[must_use]
pub fn generate_gitignore(project_type: ProjectType) -> &'static str {
    match project_type {
        ProjectType::Rust => GITIGNORE_RUST,
        ProjectType::Node => GITIGNORE_NODE,
        ProjectType::Python => GITIGNORE_PYTHON,
        ProjectType::Go => GITIGNORE_GO,
        ProjectType::Java => GITIGNORE_JAVA,
        ProjectType::Cpp => GITIGNORE_CPP,
        ProjectType::Ruby => GITIGNORE_RUBY,
        ProjectType::Swift => GITIGNORE_SWIFT,
    }
}

/// Generate combined `.gitignore` for multiple project types.
#[must_use]
pub fn generate_gitignore_multi(types: &[ProjectType]) -> String {
    let mut parts = Vec::new();
    for pt in types {
        parts.push(format!("# === {} ===\n{}", pt, generate_gitignore(*pt)));
    }
    parts.join("\n\n")
}

const GITIGNORE_RUST: &str = "\
/target/
**/*.rs.bk
Cargo.lock
*.pdb
";

const GITIGNORE_NODE: &str = "\
node_modules/
dist/
.env
.env.local
*.log
npm-debug.log*
yarn-debug.log*
yarn-error.log*
.next/
.nuxt/
";

const GITIGNORE_PYTHON: &str = "\
__pycache__/
*.py[cod]
*$py.class
*.egg-info/
dist/
build/
.venv/
venv/
.env
*.egg
.pytest_cache/
.mypy_cache/
";

const GITIGNORE_GO: &str = "\
/vendor/
*.exe
*.exe~
*.dll
*.so
*.dylib
*.test
*.out
go.work
";

const GITIGNORE_JAVA: &str = "\
*.class
*.jar
*.war
*.ear
target/
build/
.gradle/
.idea/
*.iml
out/
";

const GITIGNORE_CPP: &str = "\
build/
*.o
*.obj
*.so
*.dylib
*.dll
*.exe
*.a
*.lib
CMakeCache.txt
CMakeFiles/
cmake-build-*/
";

const GITIGNORE_RUBY: &str = "\
*.gem
*.rbc
/.config
/coverage/
/InstalledFiles
/pkg/
/spec/reports/
/test/tmp/
/test/version_tmp/
/tmp/
.bundle/
vendor/bundle
";

const GITIGNORE_SWIFT: &str = "\
.build/
Packages/
*.xcodeproj/xcuserdata/
*.xcworkspace/xcuserdata/
*.playground/timeline.xctimeline
*.playground/playground.xcworkspace
DerivedData/
.swiftpm/
";

// ── DependencyScanner ────────────────────────────────────────────────

/// A scanned dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// Dependency name.
    pub name: String,
    /// Version specifier (if available).
    pub version: Option<String>,
}

/// Scan a project directory for dependencies.
///
/// Detects the manifest file type and extracts dependency names and versions.
///
/// # Errors
///
/// Returns an error if manifest files cannot be read.
pub fn scan_dependencies(
    root: &Path,
) -> crab_common::Result<BTreeMap<ProjectType, Vec<Dependency>>> {
    let mut result = BTreeMap::new();

    // Rust — Cargo.toml
    let cargo_toml = root.join("Cargo.toml");
    if cargo_toml.exists() {
        let deps = parse_cargo_deps(&cargo_toml)?;
        if !deps.is_empty() {
            result.insert(ProjectType::Rust, deps);
        }
    }

    // Node — package.json
    let package_json = root.join("package.json");
    if package_json.exists() {
        let deps = parse_package_json_deps(&package_json)?;
        if !deps.is_empty() {
            result.insert(ProjectType::Node, deps);
        }
    }

    // Python — requirements.txt
    let requirements = root.join("requirements.txt");
    if requirements.exists() {
        let deps = parse_requirements_txt(&requirements)?;
        if !deps.is_empty() {
            result.insert(ProjectType::Python, deps);
        }
    }

    // Go — go.mod
    let go_mod = root.join("go.mod");
    if go_mod.exists() {
        let deps = parse_go_mod(&go_mod)?;
        if !deps.is_empty() {
            result.insert(ProjectType::Go, deps);
        }
    }

    Ok(result)
}

/// Parse dependencies from Cargo.toml (simplified: just [dependencies] keys).
fn parse_cargo_deps(path: &Path) -> crab_common::Result<Vec<Dependency>> {
    let content = std::fs::read_to_string(path)?;
    let table: toml::Table = content
        .parse()
        .map_err(|e| crab_common::Error::Other(format!("parse Cargo.toml: {e}")))?;

    let mut deps = Vec::new();
    if let Some(dep_table) = table.get("dependencies").and_then(|v| v.as_table()) {
        for (name, val) in dep_table {
            let version = match val {
                toml::Value::String(s) => Some(s.clone()),
                toml::Value::Table(t) => {
                    t.get("version").and_then(|v| v.as_str()).map(String::from)
                }
                _ => None,
            };
            deps.push(Dependency {
                name: name.clone(),
                version,
            });
        }
    }
    deps.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(deps)
}

/// Parse dependencies from package.json.
fn parse_package_json_deps(path: &Path) -> crab_common::Result<Vec<Dependency>> {
    let content = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| crab_common::Error::Other(format!("parse package.json: {e}")))?;

    let mut deps = Vec::new();
    for section in ["dependencies", "devDependencies"] {
        if let Some(obj) = json.get(section).and_then(|v| v.as_object()) {
            for (name, val) in obj {
                let version = val.as_str().map(String::from);
                deps.push(Dependency {
                    name: name.clone(),
                    version,
                });
            }
        }
    }
    deps.sort_by(|a, b| a.name.cmp(&b.name));
    deps.dedup_by(|a, b| a.name == b.name);
    Ok(deps)
}

/// Parse dependencies from requirements.txt.
#[allow(clippy::option_if_let_else)]
fn parse_requirements_txt(path: &Path) -> crab_common::Result<Vec<Dependency>> {
    let content = std::fs::read_to_string(path)?;
    let mut deps = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        // Format: name==version or name>=version or just name
        #[allow(clippy::option_if_let_else)]
        let (name, version) = if let Some(pos) = trimmed.find("==") {
            (&trimmed[..pos], Some(trimmed[pos + 2..].to_string()))
        } else if let Some(pos) = trimmed.find(">=") {
            (&trimmed[..pos], Some(format!(">={}", &trimmed[pos + 2..])))
        } else if let Some(pos) = trimmed.find("~=") {
            (&trimmed[..pos], Some(format!("~={}", &trimmed[pos + 2..])))
        } else {
            (trimmed, None)
        };
        deps.push(Dependency {
            name: name.trim().to_string(),
            version,
        });
    }
    Ok(deps)
}

/// Parse dependencies from go.mod.
fn parse_go_mod(path: &Path) -> crab_common::Result<Vec<Dependency>> {
    let content = std::fs::read_to_string(path)?;
    let mut deps = Vec::new();
    let mut in_require = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "require (" {
            in_require = true;
            continue;
        }
        if trimmed == ")" {
            in_require = false;
            continue;
        }
        if in_require && !trimmed.is_empty() && !trimmed.starts_with("//") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                deps.push(Dependency {
                    name: parts[0].to_string(),
                    version: Some(parts[1].to_string()),
                });
            }
        }
    }
    Ok(deps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ── ProjectType ──────────────────────────────────────────────────

    #[test]
    fn project_type_display() {
        assert_eq!(ProjectType::Rust.to_string(), "Rust");
        assert_eq!(ProjectType::Node.to_string(), "Node.js");
        assert_eq!(ProjectType::Python.to_string(), "Python");
        assert_eq!(ProjectType::Go.to_string(), "Go");
        assert_eq!(ProjectType::Java.to_string(), "Java");
        assert_eq!(ProjectType::Cpp.to_string(), "C/C++");
        assert_eq!(ProjectType::Ruby.to_string(), "Ruby");
        assert_eq!(ProjectType::Swift.to_string(), "Swift");
    }

    #[test]
    fn project_type_serde_roundtrip() {
        let json = serde_json::to_string(&ProjectType::Rust).unwrap();
        assert_eq!(json, r#""rust""#);
        let back: ProjectType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ProjectType::Rust);
    }

    // ── ProjectDetector ──────────────────────────────────────────────

    #[test]
    fn detect_rust_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Rust));
        assert!(!info.is_monorepo);
    }

    #[test]
    fn detect_node_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Node));
    }

    #[test]
    fn detect_python_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("requirements.txt"), "flask==2.0").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Python));
    }

    #[test]
    fn detect_go_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module example.com/m").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Go));
    }

    #[test]
    fn detect_java_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pom.xml"), "<project/>").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Java));
    }

    #[test]
    fn detect_cpp_project() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("CMakeLists.txt"),
            "cmake_minimum_required()",
        )
        .unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Cpp));
    }

    #[test]
    fn detect_ruby_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Gemfile"), "source 'https://rubygems.org'").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Ruby));
    }

    #[test]
    fn detect_multi_type_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.contains(&ProjectType::Rust));
        assert!(info.types.contains(&ProjectType::Node));
    }

    #[test]
    fn detect_empty_directory() {
        let dir = tempdir().unwrap();
        let info = detect_project(dir.path()).unwrap();
        assert!(info.types.is_empty());
        assert!(!info.is_monorepo);
    }

    #[test]
    fn detect_monorepo() {
        let dir = tempdir().unwrap();
        let sub_a = dir.path().join("crate-a");
        let sub_b = dir.path().join("crate-b");
        fs::create_dir_all(&sub_a).unwrap();
        fs::create_dir_all(&sub_b).unwrap();
        fs::write(sub_a.join("Cargo.toml"), "[package]").unwrap();
        fs::write(sub_b.join("Cargo.toml"), "[package]").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.is_monorepo);
        assert_eq!(info.sub_projects.len(), 2);
    }

    #[test]
    fn detect_skips_hidden_and_known_dirs() {
        let dir = tempdir().unwrap();
        let hidden = dir.path().join(".hidden");
        let nm = dir.path().join("node_modules");
        let target = dir.path().join("target");
        fs::create_dir_all(&hidden).unwrap();
        fs::create_dir_all(&nm).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(hidden.join("Cargo.toml"), "[package]").unwrap();
        fs::write(nm.join("package.json"), "{}").unwrap();
        fs::write(target.join("Cargo.toml"), "[package]").unwrap();

        let info = detect_project(dir.path()).unwrap();
        assert!(info.sub_projects.is_empty());
    }

    // ── GitignoreGenerator ───────────────────────────────────────────

    #[test]
    fn gitignore_rust_has_target() {
        let content = generate_gitignore(ProjectType::Rust);
        assert!(content.contains("/target/"));
    }

    #[test]
    fn gitignore_node_has_node_modules() {
        let content = generate_gitignore(ProjectType::Node);
        assert!(content.contains("node_modules/"));
    }

    #[test]
    fn gitignore_python_has_pycache() {
        let content = generate_gitignore(ProjectType::Python);
        assert!(content.contains("__pycache__/"));
    }

    #[test]
    fn gitignore_go_has_vendor() {
        let content = generate_gitignore(ProjectType::Go);
        assert!(content.contains("/vendor/"));
    }

    #[test]
    fn gitignore_java_has_class() {
        assert!(generate_gitignore(ProjectType::Java).contains("*.class"));
    }

    #[test]
    fn gitignore_cpp_has_build() {
        assert!(generate_gitignore(ProjectType::Cpp).contains("build/"));
    }

    #[test]
    fn gitignore_ruby_has_gem() {
        assert!(generate_gitignore(ProjectType::Ruby).contains("*.gem"));
    }

    #[test]
    fn gitignore_swift_has_build() {
        assert!(generate_gitignore(ProjectType::Swift).contains(".build/"));
    }

    #[test]
    fn gitignore_multi_combines() {
        let content = generate_gitignore_multi(&[ProjectType::Rust, ProjectType::Node]);
        assert!(content.contains("# === Rust ==="));
        assert!(content.contains("# === Node.js ==="));
        assert!(content.contains("/target/"));
        assert!(content.contains("node_modules/"));
    }

    #[test]
    fn gitignore_multi_empty() {
        let content = generate_gitignore_multi(&[]);
        assert!(content.is_empty());
    }

    // ── DependencyScanner ────────────────────────────────────────────

    #[test]
    fn scan_cargo_deps() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = { version = "1.0", features = ["full"] }
"#,
        )
        .unwrap();

        let result = scan_dependencies(dir.path()).unwrap();
        let deps = result.get(&ProjectType::Rust).unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "serde");
        assert_eq!(deps[0].version.as_deref(), Some("1.0"));
        assert_eq!(deps[1].name, "tokio");
        assert_eq!(deps[1].version.as_deref(), Some("1.0"));
    }

    #[test]
    fn scan_package_json_deps() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{
  "dependencies": { "express": "^4.0" },
  "devDependencies": { "jest": "^29.0" }
}"#,
        )
        .unwrap();

        let result = scan_dependencies(dir.path()).unwrap();
        let deps = result.get(&ProjectType::Node).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d.name == "express"));
        assert!(deps.iter().any(|d| d.name == "jest"));
    }

    #[test]
    fn scan_requirements_txt() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "flask==2.0.1\nrequests>=2.28\n# comment\nnumpy\n",
        )
        .unwrap();

        let result = scan_dependencies(dir.path()).unwrap();
        let deps = result.get(&ProjectType::Python).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "flask");
        assert_eq!(deps[0].version.as_deref(), Some("2.0.1"));
        assert_eq!(deps[1].name, "requests");
        assert!(deps[1].version.as_ref().unwrap().starts_with(">="));
        assert_eq!(deps[2].name, "numpy");
        assert!(deps[2].version.is_none());
    }

    #[test]
    fn scan_go_mod() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/m\n\ngo 1.21\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.9.1\n\tgolang.org/x/text v0.12.0\n)\n",
        )
        .unwrap();

        let result = scan_dependencies(dir.path()).unwrap();
        let deps = result.get(&ProjectType::Go).unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
        assert_eq!(deps[0].version.as_deref(), Some("v1.9.1"));
    }

    #[test]
    fn scan_no_deps_empty() {
        let dir = tempdir().unwrap();
        let result = scan_dependencies(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn scan_requirements_skips_comments_and_flags() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "# This is a comment\n-r other.txt\nflask==2.0\n",
        )
        .unwrap();

        let result = scan_dependencies(dir.path()).unwrap();
        let deps = result.get(&ProjectType::Python).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "flask");
    }

    #[test]
    fn dependency_serde_roundtrip() {
        let dep = Dependency {
            name: "serde".into(),
            version: Some("1.0".into()),
        };
        let json = serde_json::to_string(&dep).unwrap();
        let back: Dependency = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "serde");
        assert_eq!(back.version.as_deref(), Some("1.0"));
    }
}
