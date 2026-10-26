// SPDX-License-Identifier: Apache-2.0

use std::{io, process::Command};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub pid: u32,
    pub id: u64,
}

#[derive(Debug, Default)]
pub struct Windows {
    pub focused_pid: Option<u32>,
    pub windows: Vec<Window>,
}

pub fn windows() -> io::Result<Windows> {
    let output = Command::new("swaymsg").args(["-t", "get_tree"]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "swaymsg exited with {}",
            output.status
        )));
    }
    let tree = String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(windows_from_tree(&tree))
}

pub fn focus(window_id: u64) -> io::Result<()> {
    let command = focus_command(window_id);
    let status = Command::new("swaymsg")
        .args(["-t", "command", &command])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("swaymsg exited with {status}")))
    }
}

fn focus_command(window_id: u64) -> String {
    format!("[con_id={window_id}] focus")
}

fn windows_from_tree(tree: &str) -> Windows {
    let Ok(tree) = serde_json::from_str::<Value>(tree) else {
        return Windows::default();
    };
    let mut windows = Windows {
        focused_pid: find_focused_pid(&tree),
        windows: Vec::new(),
    };
    collect_windows(&tree, &mut windows.windows);
    windows
}

fn find_focused_pid(node: &Value) -> Option<u32> {
    if node.get("focused").and_then(Value::as_bool) == Some(true)
        && let Some(pid) = node.get("pid").and_then(Value::as_u64)
        && let Ok(pid) = u32::try_from(pid)
    {
        return Some(pid);
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                if let Some(pid) = find_focused_pid(child) {
                    return Some(pid);
                }
            }
        }
    }
    None
}

fn collect_windows(node: &Value, windows: &mut Vec<Window>) {
    if matches!(
        node.get("type").and_then(Value::as_str),
        Some("con" | "floating_con")
    ) && let Some(pid) = node.get("pid").and_then(Value::as_u64)
        && let Some(id) = node.get("id").and_then(Value::as_u64)
        && let Ok(pid) = u32::try_from(pid)
    {
        windows.push(Window { pid, id });
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                collect_windows(child, windows);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_focused_window_and_id() {
        let tree = r#"
        {
            "focused": false,
            "nodes": [{
                "focused": false,
                "nodes": [{
                    "id": 63,
                    "type": "con",
                    "focused": true,
                    "pid": 1234,
                    "app_id": "foot",
                    "nodes": []
                }]
            }]
        }
        "#;

        let windows = windows_from_tree(tree);

        assert_eq!(windows.focused_pid, Some(1234));
        assert_eq!(windows.windows, vec![Window { pid: 1234, id: 63 }]);
    }

    #[test]
    fn finds_focused_floating_window() {
        let tree = r#"
        {
            "focused": false,
            "nodes": [],
            "floating_nodes": [{
                "id": 7,
                "type": "floating_con",
                "focused": true,
                "pid": 42
            }]
        }
        "#;

        let windows = windows_from_tree(tree);

        assert_eq!(windows.focused_pid, Some(42));
        assert_eq!(windows.windows, vec![Window { pid: 42, id: 7 }]);
    }

    #[test]
    fn ignores_focused_workspace_without_pid() {
        let tree = r#"
        {
            "id": 1,
            "type": "workspace",
            "focused": true,
            "nodes": [{
                "id": 2,
                "type": "con",
                "focused": false,
                "pid": 1234
            }]
        }
        "#;

        let windows = windows_from_tree(tree);

        assert_eq!(windows.focused_pid, None);
        assert_eq!(windows.windows, vec![Window { pid: 1234, id: 2 }]);
    }

    #[test]
    fn ignores_invalid_tree() {
        let windows = windows_from_tree("not json");

        assert_eq!(windows.focused_pid, None);
        assert!(windows.windows.is_empty());
    }

    #[test]
    fn builds_focus_command_from_window_id() {
        assert_eq!(focus_command(63), "[con_id=63] focus");
    }
}
