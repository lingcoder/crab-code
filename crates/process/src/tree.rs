//! Process tree management (sysinfo).

use sysinfo::{Pid, System};

/// Kill a process and all of its child processes.
///
/// Uses `sysinfo` to enumerate the process tree, then kills children
/// bottom-up (leaves first) before killing the target process.
///
/// # Errors
///
/// Returns an error if the target process does not exist.
pub fn kill_tree(pid: u32) -> crab_common::Result<()> {
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let target = Pid::from_u32(pid);

    if sys.process(target).is_none() {
        return Err(crab_common::Error::Other(format!(
            "process {pid} not found"
        )));
    }

    // Collect all descendants (children, grandchildren, etc.)
    let descendants = collect_descendants(&sys, target);

    // Kill bottom-up: children first, then the target
    for &desc_pid in descendants.iter().rev() {
        if let Some(proc) = sys.process(desc_pid) {
            proc.kill();
        }
    }

    // Kill the target itself
    if let Some(proc) = sys.process(target) {
        proc.kill();
    }

    Ok(())
}

/// Recursively collect all descendant PIDs of `root` in BFS order.
fn collect_descendants(sys: &System, root: Pid) -> Vec<Pid> {
    let mut result = Vec::new();
    let mut queue = vec![root];

    while let Some(parent) = queue.pop() {
        for (pid, proc) in sys.processes() {
            if proc.parent() == Some(parent) && *pid != root {
                result.push(*pid);
                queue.push(*pid);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_tree_nonexistent_process() {
        // PID 0 or a very high PID should not exist as a user process
        let result = kill_tree(u32::MAX);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn collect_descendants_empty() {
        let sys = System::new();
        // No processes loaded, so no descendants
        let descendants = collect_descendants(&sys, Pid::from_u32(1));
        assert!(descendants.is_empty());
    }

    #[tokio::test]
    async fn kill_tree_spawned_process() {
        // Spawn a long-running child and kill its tree
        let child = if cfg!(windows) {
            tokio::process::Command::new("cmd")
                .args(["/C", "ping -n 100 127.0.0.1 >nul"])
                .spawn()
        } else {
            tokio::process::Command::new("sleep").arg("100").spawn()
        };

        let child = child.expect("failed to spawn test process");
        let pid = child.id().expect("no pid");

        // Give process a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let result = kill_tree(pid);
        assert!(result.is_ok());
    }
}
