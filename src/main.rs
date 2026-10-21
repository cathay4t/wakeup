mod monitor;
mod notify;
mod opencode;

use std::{thread, time::Duration};

use monitor::{Monitor, WATCHED_COMMANDS};

const POLL_INTERVAL: Duration = Duration::from_secs(30);

fn main() {
    eprintln!(
        "wakeup: monitoring {} and opencode every {}s",
        WATCHED_COMMANDS.join(", "),
        POLL_INTERVAL.as_secs()
    );

    let mut monitor = Monitor::new();
    let mut opencode = opencode::Watcher::new();
    let mut last_opencode_error = None;
    loop {
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

        thread::sleep(POLL_INTERVAL);
    }
}
