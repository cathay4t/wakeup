// SPDX-License-Identifier: Apache-2.0

use std::{io, process::Command};

use serde_json::Value;

pub fn focused_window_pid() -> io::Result<Option<u32>> {
    let output = Command::new("swaymsg").args(["-t", "get_tree"]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "swaymsg exited with {}",
            output.status
        )));
    }
    let tree = String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(focused_window_pid_from_tree(&tree))
}

fn focused_window_pid_from_tree(tree: &str) -> Option<u32> {
    let tree: Value = serde_json::from_str(tree).ok()?;
    find_focused_pid(&tree)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pid_of_focused_window() {
        let tree = r#"
        {
            "focused": false,
            "nodes": [{
                "focused": false,
                "nodes": [{
                    "focused": true,
                    "pid": 1234,
                    "app_id": "foot",
                    "nodes": []
                }]
            }]
        }
        "#;

        assert_eq!(focused_window_pid_from_tree(tree), Some(1234));
    }

    #[test]
    fn finds_focused_floating_window() {
        let tree = r#"
        {
            "focused": false,
            "nodes": [],
            "floating_nodes": [{
                "focused": true,
                "pid": 42
            }]
        }
        "#;

        assert_eq!(focused_window_pid_from_tree(tree), Some(42));
    }

    #[test]
    fn ignores_focused_workspace_without_pid() {
        let tree = r#"
        {
            "focused": true,
            "nodes": [{"focused": false, "pid": 1234}]
        }
        "#;

        assert_eq!(focused_window_pid_from_tree(tree), None);
    }

    #[test]
    fn ignores_invalid_tree() {
        assert_eq!(focused_window_pid_from_tree("not json"), None);
    }
}
