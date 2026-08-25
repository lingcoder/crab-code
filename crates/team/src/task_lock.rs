//! File-backed [`TaskList`] with OS-level exclusive locking.
//!
//! `Arc<Mutex<TaskList>>` (from [`crab_core::task::shared_task_list`]) is
//! fine for single-process teammates. Once teammates run in separate
//! processes — tmux panes, remote agents, background workers — two
//! teammates could race on `claim_task` and each believe they own the same
//! [`Task`]. This module serialises those claims through an OS file lock.
//!
//! Uses [`crab_utils::lock`] (cross-platform flock/LockFileEx wrapper) — the library
//! choice follows the "prefer existing crates" rule.
//!
//! # Lock protocol
//!
//! The task list lives at `<path>` (JSON). A sibling `<path>.lock` file
//! holds an exclusive [`crab_utils::lock::RwLock`] write guard for the duration of
//! every mutation. Readers use the same exclusive lock to avoid read/write
//! races — task claiming is always a read-modify-write cycle.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crab_utils::lock::RwLock;

use crab_core::task::{Task, TaskList};

/// Load a [`TaskList`] from `path`, returning an empty list when the file
/// does not exist. Never takes the lock — intended for tests and snapshots.
/// Use [`with_locked`] for any read-modify-write sequence.
pub fn load_from_file(path: &Path) -> std::io::Result<TaskList> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(TaskList::default()),
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TaskList::default()),
        Err(e) => Err(e),
    }
}

/// Serialise `list` to `path` atomically (write to a temp sibling then rename).
/// Caller must already hold the file lock.
fn save_to_file(path: &Path, list: &TaskList) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(list).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Execute `f` under an OS-exclusive lock over `<path>.lock`, with `path`
/// deserialised into `&mut TaskList` before the call and re-serialised after.
///
/// This is the atomic primitive for Swarm / Coordinator Mode when teammates
/// live in separate processes. All mutations — `create`, `update`,
/// `claim_task` — must flow through this helper so that no two processes
/// observe a stale view.
pub fn with_locked<R>(path: &Path, f: impl FnOnce(&mut TaskList) -> R) -> std::io::Result<R> {
    let lock_path = lock_path_for(path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    let mut rw = RwLock::new(lock_file);
    // Exclusive lock — blocks other processes until dropped at scope end.
    let _guard = rw.write()?;

    let mut list = load_from_file(path)?;
    let result = f(&mut list);
    save_to_file(path, &list)?;
    Ok(result)
}

/// Claim the next available task for `agent_id` under an exclusive lock.
///
/// Returns `Ok(Some(task))` when a task was claimed (its `owner` is set to
/// `agent_id` on disk), `Ok(None)` when no task was available. Concurrent
/// callers across processes are serialised by the file lock — at most one
/// caller claims any given task.
pub fn claim_task(path: &Path, agent_id: &str) -> std::io::Result<Option<Task>> {
    with_locked(path, |list| {
        let target_id = list.available_tasks().first().map(|t| t.id.clone())?;
        list.update(
            &target_id,
            None,
            None,
            None,
            Some(agent_id.to_string()),
            None,
            None,
        );
        list.get(&target_id).cloned()
    })
}

/// Compute the sibling lock-file path for a given task-list path.
/// `foo/tasks.json` → `foo/tasks.json.lock`.
fn lock_path_for(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".lock");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::task::TaskStatus;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let list = load_from_file(&path).unwrap();
        assert_eq!(list.list().len(), 0);
    }

    #[test]
    fn roundtrip_through_with_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");

        with_locked(&path, |list| {
            list.create("Write docs".into(), "Write user-facing docs".into());
        })
        .unwrap();

        let reloaded = load_from_file(&path).unwrap();
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].subject, "Write docs");
    }

    #[test]
    fn claim_sets_owner_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");

        let id = with_locked(&path, |list| list.create("Task A".into(), "desc".into())).unwrap();

        let claimed = claim_task(&path, "alice").unwrap().unwrap();
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.owner.as_deref(), Some("alice"));

        // Persisted?
        let reloaded = load_from_file(&path).unwrap();
        assert_eq!(reloaded.get(&id).unwrap().owner.as_deref(), Some("alice"));
    }

    #[test]
    fn claim_returns_none_when_no_tasks_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        assert!(claim_task(&path, "alice").unwrap().is_none());
    }

    #[test]
    fn claim_skips_already_owned_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");

        with_locked(&path, |list| {
            list.create("T1".into(), String::new());
            list.create("T2".into(), String::new());
        })
        .unwrap();

        let first = claim_task(&path, "alice").unwrap().unwrap();
        let second = claim_task(&path, "bob").unwrap().unwrap();

        assert_ne!(first.id, second.id, "must claim different tasks");
        assert_eq!(first.owner.as_deref(), Some("alice"));
        assert_eq!(second.owner.as_deref(), Some("bob"));

        // Third claim exhausts the list.
        assert!(claim_task(&path, "carol").unwrap().is_none());
    }

    #[test]
    fn claim_is_atomic_across_threads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");

        // Seed 10 tasks.
        with_locked(&path, |list| {
            for i in 0..10 {
                list.create(format!("T{i}"), String::new());
            }
        })
        .unwrap();

        // 10 threads race to claim. Each should succeed exactly once on a
        // distinct task; no two threads may claim the same id. We must
        // `collect()` the spawn results before joining so the barrier can
        // actually fire (otherwise each thread would spawn-then-join
        // serially with no contention).
        let barrier = Arc::new(Barrier::new(10));
        let mut handles = Vec::with_capacity(10);
        for i in 0..10 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                claim_task(&path, &format!("agent-{i}"))
                    .unwrap()
                    .expect("every racing agent should claim something")
                    .id
            }));
        }

        let mut claimed_ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        claimed_ids.sort();
        claimed_ids.dedup();
        assert_eq!(
            claimed_ids.len(),
            10,
            "all claims must land on distinct tasks"
        );
    }

    #[test]
    fn with_locked_propagates_modifications() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");

        let id = with_locked(&path, |list| list.create("T".into(), String::new())).unwrap();

        with_locked(&path, |list| {
            list.update(
                &id,
                Some(TaskStatus::Completed),
                None,
                None,
                None,
                None,
                None,
            );
        })
        .unwrap();

        let reloaded = load_from_file(&path).unwrap();
        // Completed tasks are still visible via list().
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].status, TaskStatus::Completed);
    }

    #[test]
    fn lock_path_appends_suffix() {
        assert_eq!(
            lock_path_for(Path::new("/tmp/tasks.json")),
            PathBuf::from("/tmp/tasks.json.lock")
        );
    }
}
