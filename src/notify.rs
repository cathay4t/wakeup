use std::{io, process::Command, time::Duration};

use crate::monitor::FinishedTask;

pub fn send(task: &FinishedTask) -> io::Result<()> {
    let (summary, body) = message(task);
    let status = Command::new("notify-send")
        .arg("--app-name=wakeup")
        .arg("--urgency=normal")
        .arg("--")
        .arg(&summary)
        .arg(&body)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "notify-send exited with {status}"
        )))
    }
}

fn message(task: &FinishedTask) -> (String, String) {
    (
        format!(
            "{} finished ({})",
            task.name,
            format_duration(task.duration)
        ),
        format!("Command: {}", task.command_line),
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
            message(&task("wget -c https://example.com/file"));

        assert_eq!(summary, "wget finished (1m 01s)");
        assert_eq!(body, "Command: wget -c https://example.com/file");
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
