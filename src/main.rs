mod monitor;
mod notify;

use std::{thread, time::Duration};

use monitor::{Monitor, WATCHED_COMMANDS};

const POLL_INTERVAL: Duration = Duration::from_secs(5);

fn main() {
    eprintln!(
        "wakeup: monitoring {} every {}s",
        WATCHED_COMMANDS.join(", "),
        POLL_INTERVAL.as_secs()
    );

    let mut monitor = Monitor::new();
    loop {
        match monitor.scan() {
            Ok(finished) => {
                for task in finished {
                    if let Err(error) = notify::send(&task) {
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
        thread::sleep(POLL_INTERVAL);
    }
}
