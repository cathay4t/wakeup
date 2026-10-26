// SPDX-License-Identifier: Apache-2.0

mod monitor;
mod notify;
mod opencode;
mod sway;

use std::{
    thread,
    time::{Duration, Instant},
};

use monitor::{Monitor, WATCHED_COMMANDS};

const TASK_POLL_INTERVAL: Duration = Duration::from_secs(30);
const OPENCODE_POLL_INTERVAL: Duration = Duration::from_secs(5);

fn main() {
    eprintln!(
        "wakeup: monitoring {} every {}s and opencode every {}s",
        WATCHED_COMMANDS.join(", "),
        TASK_POLL_INTERVAL.as_secs(),
        OPENCODE_POLL_INTERVAL.as_secs()
    );

    let mut monitor = Monitor::new();
    let mut opencode = opencode::Watcher::new();
    let mut last_opencode_error = None;
    let mut last_task_scan: Option<Instant> = None;
    loop {
        if last_task_scan
            .is_none_or(|last| last.elapsed() >= TASK_POLL_INTERVAL)
        {
            match monitor.scan() {
                Ok(finished) => {
                    for task in finished {
                        if let Err(error) = notify::send_task(&task) {
                            eprintln!(
                                "wakeup: failed to notify about {}: {error}",
                                task.name
                            );
                        }
                    }
                }
                Err(error) => {
                    eprintln!("wakeup: failed to scan processes: {error}");
                }
            }
            last_task_scan = Some(Instant::now());
        }

        match opencode.scan() {
            Ok(ready) => {
                last_opencode_error = None;
                for session in ready {
                    if let Err(error) = notify::send_opencode(&session) {
                        eprintln!(
                            "wakeup: failed to notify about opencode: {error}"
                        );
                    }
                }
            }
            Err(error) => {
                let message = error.to_string();
                if last_opencode_error.as_deref() != Some(message.as_str()) {
                    eprintln!("wakeup: failed to monitor opencode: {message}");
                    last_opencode_error = Some(message);
                }
            }
        }

        thread::sleep(OPENCODE_POLL_INTERVAL);
    }
}
