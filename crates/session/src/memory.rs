use std::path::PathBuf;

/// A single memory file with frontmatter metadata.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub name: String,
    pub description: String,
    pub memory_type: String,
    pub body: String,
    /// Filename (without directory).
    pub filename: String,
}

/// An entry in the `MEMORY.md` index.
#[derive(Debug, Clone)]
pub struct MemoryIndexEntry {
    pub title: String,
    pub filename: String,
    pub description: String,
}

/// File-based memory system — reads/writes `~/.crab/memory/`.
///
/// Layout:
/// ```text
/// ~/.crab/memory/
///   MEMORY.md          # Index file (one-line pointers)
///   user_role.md       # Individual memory files with frontmatter
///   feedback_style.md
///   project_auth.md
/// ```
pub struct MemoryStore {
    pub path: PathBuf,
}

impl MemoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn ensure_dir(&self) -> crab_common::Result<()> {
        std::fs::create_dir_all(&self.path)?;
        Ok(())
    }

    /// Save a memory file (overwrites if exists).
    pub fn save(&self, filename: &str, content: &str) -> crab_common::Result<()> {
        self.ensure_dir()?;
        std::fs::write(self.path.join(filename), content)?;
        Ok(())
    }

    /// Load a memory file by filename. Returns `None` if not found.
    pub fn load(&self, filename: &str) -> crab_common::Result<Option<String>> {
        let path = self.path.join(filename);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(Some(content))
    }

    /// Delete a memory file.
    pub fn delete(&self, filename: &str) -> crab_common::Result<()> {
        let path = self.path.join(filename);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Parse a memory file's frontmatter and body.
    pub fn parse_memory_file(content: &str) -> Option<MemoryFile> {
        // Expect: ---\n<frontmatter>\n---\n<body>
        let content = content.trim_start();
        if !content.starts_with("---") {
            return None;
        }
        let after_first = &content[3..];
        let end_idx = after_first.find("\n---")?;
        let frontmatter = &after_first[..end_idx];
        let body = after_first[end_idx + 4..].trim().to_string();

        let mut name = String::new();
        let mut description = String::new();
        let mut memory_type = String::new();

        for line in frontmatter.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("name:") {
                name = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("description:") {
                description = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("type:") {
                memory_type = val.trim().to_string();
            }
        }

        if name.is_empty() {
            return None;
        }

        Some(MemoryFile {
            name,
            description,
            memory_type,
            body,
            filename: String::new(), // caller fills this in
        })
    }

    /// Load and parse all memory files (excluding `MEMORY.md`).
    pub fn load_all(&self) -> crab_common::Result<Vec<MemoryFile>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let mut memories = Vec::new();
        for entry in std::fs::read_dir(&self.path)? {
            let entry = entry?;
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            if fname == "MEMORY.md" || !fname.ends_with(".md") {
                continue;
            }
            let content = std::fs::read_to_string(entry.path())?;
            if let Some(mut mem) = Self::parse_memory_file(&content) {
                mem.filename = fname.to_string();
                memories.push(mem);
            }
        }
        memories.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(memories)
    }

    /// Parse the `MEMORY.md` index file.
    pub fn load_index(&self) -> crab_common::Result<Vec<MemoryIndexEntry>> {
        let index_path = self.path.join("MEMORY.md");
        if !index_path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&index_path)?;
        Ok(Self::parse_index(&content))
    }

    /// Parse index entries from `MEMORY.md` content.
    ///
    /// Expected format: `- [Title](file.md) -- one-line hook`
    fn parse_index(content: &str) -> Vec<MemoryIndexEntry> {
        let mut entries = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if !line.starts_with("- [") {
                continue;
            }
            // Parse: - [Title](file.md) -- description
            let Some(title_end) = line.find("](") else {
                continue;
            };
            let title = &line[3..title_end];
            let rest = &line[title_end + 2..];
            let Some(link_end) = rest.find(')') else {
                continue;
            };
            let filename = &rest[..link_end];
            let description = rest[link_end + 1..]
                .trim()
                .trim_start_matches("—")
                .trim_start_matches("--")
                .trim()
                .to_string();

            entries.push(MemoryIndexEntry {
                title: title.to_string(),
                filename: filename.to_string(),
                description,
            });
        }
        entries
    }

    /// Save the `MEMORY.md` index file.
    pub fn save_index(&self, entries: &[MemoryIndexEntry]) -> crab_common::Result<()> {
        use std::fmt::Write;
        self.ensure_dir()?;
        let mut content = String::new();
        for entry in entries {
            let _ = writeln!(
                content,
                "- [{}]({}) — {}",
                entry.title, entry.filename, entry.description
            );
        }
        std::fs::write(self.path.join("MEMORY.md"), content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory_file_basic() {
        let content = r#"---
name: Test memory
description: A test memory file
type: user
---

This is the body content.

**Why:** because testing.
"#;
        let mem = MemoryStore::parse_memory_file(content).unwrap();
        assert_eq!(mem.name, "Test memory");
        assert_eq!(mem.description, "A test memory file");
        assert_eq!(mem.memory_type, "user");
        assert!(mem.body.contains("This is the body content."));
        assert!(mem.body.contains("**Why:** because testing."));
    }

    #[test]
    fn parse_memory_file_no_frontmatter() {
        assert!(MemoryStore::parse_memory_file("just some text").is_none());
    }

    #[test]
    fn parse_memory_file_no_name() {
        let content = "---\ndescription: no name\ntype: user\n---\nbody";
        assert!(MemoryStore::parse_memory_file(content).is_none());
    }

    #[test]
    fn parse_index_entries() {
        let content = "- [No telemetry](project_no_telemetry.md) — All data stays local\n\
                        - [Config path](project_config.md) — Use ~/.crab/ only\n";
        let entries = MemoryStore::parse_index(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "No telemetry");
        assert_eq!(entries[0].filename, "project_no_telemetry.md");
        assert_eq!(entries[0].description, "All data stays local");
        assert_eq!(entries[1].title, "Config path");
    }

    #[test]
    fn parse_index_skips_non_entries() {
        let content = "# Memory Index\n\nSome text\n- [Valid](file.md) — desc\n";
        let entries = MemoryStore::parse_index(content);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn save_and_load_memory() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());

        let content = "---\nname: Test\ndescription: test\ntype: user\n---\n\nBody here.";
        store.save("test_memory.md", content).unwrap();

        let loaded = store.load("test_memory.md").unwrap().unwrap();
        assert_eq!(loaded, content);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());
        assert!(store.load("nope.md").unwrap().is_none());
    }

    #[test]
    fn load_all_parses_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());

        store
            .save(
                "user_role.md",
                "---\nname: User role\ndescription: Role info\ntype: user\n---\n\nSenior dev.",
            )
            .unwrap();
        store
            .save(
                "feedback_style.md",
                "---\nname: Style feedback\ndescription: Code style\ntype: feedback\n---\n\nBe terse.",
            )
            .unwrap();
        // MEMORY.md should be excluded
        store
            .save("MEMORY.md", "- [User role](user_role.md)")
            .unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].filename, "feedback_style.md");
        assert_eq!(all[1].filename, "user_role.md");
    }

    #[test]
    fn save_and_load_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());

        let entries = vec![
            MemoryIndexEntry {
                title: "Role".into(),
                filename: "user_role.md".into(),
                description: "User's role info".into(),
            },
            MemoryIndexEntry {
                title: "Style".into(),
                filename: "feedback_style.md".into(),
                description: "Code style prefs".into(),
            },
        ];
        store.save_index(&entries).unwrap();

        let loaded = store.load_index().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].title, "Role");
        assert_eq!(loaded[1].title, "Style");
    }

    #[test]
    fn delete_memory() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());

        store.save("temp.md", "content").unwrap();
        assert!(store.load("temp.md").unwrap().is_some());

        store.delete("temp.md").unwrap();
        assert!(store.load("temp.md").unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());
        store.delete("nonexistent.md").unwrap(); // should not error
    }

    #[test]
    fn parse_memory_file_empty_body() {
        let content = "---\nname: Empty body\ndescription: test\ntype: user\n---\n";
        let mem = MemoryStore::parse_memory_file(content).unwrap();
        assert_eq!(mem.name, "Empty body");
        assert!(mem.body.is_empty());
    }

    #[test]
    fn parse_memory_file_incomplete_frontmatter() {
        // Missing closing ---
        let content = "---\nname: Test\ndescription: test\ntype: user\nno closing";
        assert!(MemoryStore::parse_memory_file(content).is_none());
    }

    #[test]
    fn parse_memory_file_extra_fields_ignored() {
        let content =
            "---\nname: Test\ndescription: test\ntype: user\nunknown_field: value\n---\n\nBody.";
        let mem = MemoryStore::parse_memory_file(content).unwrap();
        assert_eq!(mem.name, "Test");
    }

    #[test]
    fn load_all_skips_non_md_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());

        store
            .save(
                "user_role.md",
                "---\nname: Role\ndescription: test\ntype: user\n---\n\nBody.",
            )
            .unwrap();
        // Save a non-md file
        std::fs::write(dir.path().join("notes.txt"), "not a memory file").unwrap();
        // Save a file without valid frontmatter
        store.save("invalid.md", "no frontmatter here").unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].filename, "user_role.md");
    }

    #[test]
    fn load_all_nonexistent_dir_returns_empty() {
        let store = MemoryStore::new(PathBuf::from("/nonexistent/memory/dir"));
        let all = store.load_all().unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn load_index_nonexistent_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());
        let entries = store.load_index().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());

        store.save("test.md", "original").unwrap();
        store.save("test.md", "updated").unwrap();
        let loaded = store.load("test.md").unwrap().unwrap();
        assert_eq!(loaded, "updated");
    }

    #[test]
    fn parse_index_with_dash_dash_separator() {
        let content = "- [Title](file.md) -- description with dashes\n";
        let entries = MemoryStore::parse_index(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Title");
        assert_eq!(entries[0].filename, "file.md");
        assert_eq!(entries[0].description, "description with dashes");
    }

    #[test]
    fn save_index_then_load_index_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());

        let entries = vec![MemoryIndexEntry {
            title: "First".into(),
            filename: "first.md".into(),
            description: "First memory".into(),
        }];
        store.save_index(&entries).unwrap();

        let loaded = store.load_index().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "First");
        assert_eq!(loaded[0].filename, "first.md");
        assert_eq!(loaded[0].description, "First memory");
    }

    #[test]
    fn save_creates_directory_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("deep").join("memory");
        let store = MemoryStore::new(nested.clone());

        store.save("test.md", "content").unwrap();
        assert!(nested.join("test.md").exists());
    }
}
