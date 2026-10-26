// SPDX-License-Identifier: Apache-2.0

use std::{io, process::Command, thread, time::Duration};

use crate::{monitor::FinishedTask, opencode::ReadySession};

pub fn send_task(task: &FinishedTask) -> io::Result<()> {
    let (summary, body) = task_message(task);
    send(&summary, &body)
}

pub fn send_opencode(session: &ReadySession) -> io::Result<()> {
    let (summary, body) = opencode_message(session);
    let window_id = session.window_id;
    thread::Builder::new()
        .name("wakeup-opencode-notification".to_owned())
        .spawn(move || {
            let result = match window_id {
                Some(window_id) => {
                    send_with_window_action(&summary, &body, window_id)
                }
                None => send(&summary, &body),
            };
            if let Err(error) = result {
                eprintln!("wakeup: failed to notify about opencode: {error}");
            }
        })
        .map(|_| ())
}

fn send(summary: &str, body: &str) -> io::Result<()> {
    let status = Command::new("notify-send")
        .arg("--app-name=wakeup")
        .arg("--urgency=normal")
        .arg("--")
        .arg(summary)
        .arg(body)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "notify-send exited with {status}"
        )))
    }
}

fn send_with_window_action(
    summary: &str,
    body: &str,
    window_id: u64,
) -> io::Result<()> {
    let output = Command::new("notify-send")
        .arg("--app-name=wakeup")
        .arg("--urgency=normal")
        .arg("--wait")
        .arg("--action=default=Switch to window")
        .arg("--")
        .arg(summary)
        .arg(body)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "notify-send exited with {}",
            output.status
        )));
    }
    if action_was_invoked(&output.stdout, "default") {
        crate::sway::focus(window_id).map_err(|error| {
            io::Error::other(format!(
                "failed to focus window {window_id}: {error}"
            ))
        })?;
    }
    Ok(())
}

fn action_was_invoked(stdout: &[u8], action: &str) -> bool {
    std::str::from_utf8(stdout)
        .is_ok_and(|stdout| stdout.lines().any(|line| line.trim() == action))
}

fn task_message(task: &FinishedTask) -> (String, String) {
    (
        format!(
            "{} finished ({})",
            task.name,
            format_duration(task.duration)
        ),
        format!("Command: {}", task.command_line),
    )
}

fn opencode_message(session: &ReadySession) -> (String, String) {
    (
        "opencode is waiting for input".to_owned(),
        format!("{}\n{}", session.title, session.directory),
    )
}

fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let days = total / 86_400;
    let hours = total % 86_400 / 3_600;
    let minutes = total % 3_600 / 60;
    let seconds = total % 60;

    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m {seconds:02}s")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(command_line: &str) -> FinishedTask {
        FinishedTask {
            name: "wget".to_owned(),
            command_line: command_line.to_owned(),
            duration: Duration::from_secs(61),
        }
    }

    #[test]
    fn message_contains_full_command_line() {
        let (summary, body) =
            task_message(&task("wget -c https://example.com/file"));

        assert_eq!(summary, "wget finished (1m 01s)");
        assert_eq!(body, "Command: wget -c https://example.com/file");
    }

    #[test]
    fn opencode_message_contains_session_context() {
        let session = ReadySession {
            title: "Fix the build".to_owned(),
            directory: "/home/user/project".to_owned(),
            window_id: None,
        };

        let (summary, body) = opencode_message(&session);

        assert_eq!(summary, "opencode is waiting for input");
        assert_eq!(body, "Fix the build\n/home/user/project");
    }

    #[test]
    fn recognizes_invoked_default_action() {
        assert!(action_was_invoked(b"default\n", "default"));
        assert!(!action_was_invoked(b"", "default"));
        assert!(!action_was_invoked(b"other\n", "default"));
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_secs(5)), "5s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m 00s");
        assert_eq!(format_duration(Duration::from_secs(3_661)), "1h 01m 01s");
        assert_eq!(
            format_duration(Duration::from_secs(90_061)),
            "1d 01h 01m 01s"
        );
    }
}
