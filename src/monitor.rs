use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

pub const WATCHED_COMMANDS: &[&str] = &["make", "tar", "wget", "xz"];
const MIN_RUNTIME: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct FinishedTask {
    pub name: String,
    pub command_line: String,
    pub duration: Duration,
}

struct TrackedProcess {
    start_ticks: u64,
    name: String,
    command_line: String,
}

struct ProcessStat {
    comm: String,
    state: char,
    start_ticks: u64,
}

pub struct Monitor {
    proc_root: PathBuf,
    ticks_per_second: u64,
    tracked: HashMap<u32, TrackedProcess>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            ticks_per_second: clock_ticks_per_second(),
            tracked: HashMap::new(),
        }
    }

    #[cfg(test)]
    fn with_proc_root(proc_root: PathBuf) -> Self {
        Self {
            proc_root,
            ticks_per_second: 100,
            tracked: HashMap::new(),
        }
    }

    pub fn scan(&mut self) -> io::Result<Vec<FinishedTask>> {
        let now = read_uptime(&self.proc_root)?;
        self.scan_at(now)
    }

    fn scan_at(&mut self, now: Duration) -> io::Result<Vec<FinishedTask>> {
        let mut seen = HashMap::new();
        for entry in fs::read_dir(&self.proc_root)? {
            let Ok(entry) = entry else {
                continue;
            };
            let Some(pid) =
                entry.file_name().to_string_lossy().parse::<u32>().ok()
            else {
                continue;
            };
            if let Some(stat) = read_stat(&entry.path().join("stat")) {
                seen.insert(pid, stat);
            }
        }

        let mut finished = Vec::new();
        let tracked_pids: Vec<u32> = self.tracked.keys().copied().collect();
        for pid in tracked_pids {
            if let Some(stat) = seen.get(&pid) {
                let same_process =
                    self.tracked.get(&pid).is_some_and(|tracked| {
                        tracked.start_ticks == stat.start_ticks
                    });
                if (!same_process || is_dead(stat.state))
                    && let Some(task) = self.finish(pid, now)
                {
                    finished.push(task);
                }
            } else {
                let path = self.proc_root.join(pid.to_string());
                if matches!(path.try_exists(), Ok(false))
                    && let Some(task) = self.finish(pid, now)
                {
                    finished.push(task);
                }
            }
        }

        for (pid, stat) in seen {
            if self.tracked.contains_key(&pid) {
                continue;
            }
            if is_dead(stat.state) || !is_watched(&stat.comm) {
                continue;
            }
            let cmdline_path =
                self.proc_root.join(pid.to_string()).join("cmdline");
            let command_line = read_command_line(&cmdline_path)
                .unwrap_or_else(|| stat.comm.clone());
            self.tracked.insert(
                pid,
                TrackedProcess {
                    start_ticks: stat.start_ticks,
                    name: stat.comm,
                    command_line,
                },
            );
        }

        Ok(finished)
    }

    fn finish(&mut self, pid: u32, now: Duration) -> Option<FinishedTask> {
        let tracked = self.tracked.remove(&pid)?;
        let duration = elapsed_since_start(
            now,
            tracked.start_ticks,
            self.ticks_per_second,
        );
        if duration <= MIN_RUNTIME {
            return None;
        }
        Some(FinishedTask {
            name: tracked.name,
            command_line: tracked.command_line,
            duration,
        })
    }
}

fn is_watched(comm: &str) -> bool {
    WATCHED_COMMANDS.contains(&comm)
}

fn is_dead(state: char) -> bool {
    matches!(state, 'Z' | 'X' | 'x')
}

fn clock_ticks_per_second() -> u64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks)
        .ok()
        .filter(|ticks| *ticks > 0)
        .unwrap_or(100)
}

fn read_uptime(proc_root: &Path) -> io::Result<Duration> {
    let data = fs::read_to_string(proc_root.join("uptime"))?;
    parse_uptime(&data).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid /proc/uptime")
    })
}

fn parse_uptime(data: &str) -> Option<Duration> {
    let seconds: f64 = data.split_whitespace().next()?.parse().ok()?;
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return None;
    }
    Duration::try_from_secs_f64(seconds).ok()
}

fn elapsed_since_start(
    now: Duration,
    start_ticks: u64,
    ticks_per_second: u64,
) -> Duration {
    let start =
        Duration::from_secs_f64(start_ticks as f64 / ticks_per_second as f64);
    now.saturating_sub(start)
}

fn read_stat(path: &Path) -> Option<ProcessStat> {
    let data = fs::read(path).ok()?;
    parse_proc_stat(&String::from_utf8_lossy(&data))
}

fn parse_proc_stat(data: &str) -> Option<ProcessStat> {
    let open = data.find('(')?;
    let close = data.rfind(')')?;
    if close <= open {
        return None;
    }
    let comm = data.get(open + 1..close)?;
    let rest = data.get(close + 1..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let state = fields.first()?.chars().next()?;
    let start_ticks = fields.get(19)?.parse().ok()?;
    Some(ProcessStat {
        comm: comm.to_owned(),
        state,
        start_ticks,
    })
}

fn read_command_line(path: &Path) -> Option<String> {
    let data = fs::read(path).ok()?;
    command_line_from_bytes(&data)
}

fn command_line_from_bytes(data: &[u8]) -> Option<String> {
    if data.is_empty() {
        return None;
    }
    let mut parts: Vec<&[u8]> = data.split(|byte| *byte == 0).collect();
    while parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    if parts.is_empty() {
        return None;
    }
    Some(
        parts
            .into_iter()
            .map(|part| quote_argument(&String::from_utf8_lossy(part)))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn quote_argument(argument: &str) -> String {
    let safe = !argument.is_empty()
        && argument.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'_'
                        | b'.'
                        | b'/'
                        | b':'
                        | b'='
                        | b'@'
                        | b'%'
                        | b'+'
                        | b','
                )
        });
    if safe {
        argument.to_owned()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static NEXT_ID: AtomicU32 = AtomicU32::new(0);

    fn stat_line(
        pid: u32,
        comm: &str,
        state: char,
        start_ticks: u64,
    ) -> String {
        let mut fields = vec![state.to_string()];
        fields.resize(19, "0".to_owned());
        fields.push(start_ticks.to_string());
        format!("{pid} ({comm}) {}", fields.join(" "))
    }

    struct FakeProc {
        root: PathBuf,
    }

    impl FakeProc {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("wakeup-proc-{}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn add(
            &self,
            pid: u32,
            comm: &str,
            state: char,
            start_ticks: u64,
            args: &[&str],
        ) {
            let dir = self.root.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("stat"),
                stat_line(pid, comm, state, start_ticks),
            )
            .unwrap();
            let mut cmdline = Vec::new();
            for arg in args {
                cmdline.extend_from_slice(arg.as_bytes());
                cmdline.push(0);
            }
            fs::write(dir.join("cmdline"), cmdline).unwrap();
        }

        fn remove(&self, pid: u32) {
            fs::remove_dir_all(self.root.join(pid.to_string())).unwrap();
        }
    }

    impl Drop for FakeProc {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn parses_proc_stat_with_parentheses_in_comm() {
        let stat =
            parse_proc_stat(&stat_line(123, "name) with (parens", 'S', 4_567))
                .unwrap();

        assert_eq!(stat.comm, "name) with (parens");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.start_ticks, 4_567);
    }

    #[test]
    fn parses_uptime() {
        assert_eq!(
            parse_uptime("148908.53 3227959.55\n"),
            Some(Duration::from_secs_f64(148_908.53))
        );
        assert_eq!(parse_uptime("bad"), None);
    }

    #[test]
    fn command_line_preserves_all_arguments() {
        let command = command_line_from_bytes(
            b"wget\0-c\0https://example.com/a b\0\0tail\0",
        )
        .unwrap();

        assert_eq!(command, "wget -c 'https://example.com/a b' '' tail");
    }

    #[test]
    fn ignores_tasks_shorter_than_a_minute() {
        let proc = FakeProc::new();
        proc.add(42, "wget", 'S', 0, &["wget", "-c", "url"]);
        let mut monitor = Monitor::with_proc_root(proc.root.clone());

        assert!(monitor.scan_at(Duration::from_secs(10)).unwrap().is_empty());
        proc.remove(42);

        assert!(monitor.scan_at(Duration::from_secs(60)).unwrap().is_empty());
    }

    #[test]
    fn detects_finished_long_task() {
        let proc = FakeProc::new();
        proc.add(42, "wget", 'S', 0, &["wget", "-c", "url"]);
        let mut monitor = Monitor::with_proc_root(proc.root.clone());

        assert!(monitor.scan_at(Duration::from_secs(10)).unwrap().is_empty());
        proc.remove(42);

        let finished = monitor.scan_at(Duration::from_secs(61)).unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].name, "wget");
        assert_eq!(finished[0].command_line, "wget -c url");
        assert_eq!(finished[0].duration, Duration::from_secs(61));
    }

    #[test]
    fn detects_finished_task_that_started_before_monitor() {
        let proc = FakeProc::new();
        proc.add(42, "make", 'S', 0, &["make", "-j8", "all"]);
        let mut monitor = Monitor::with_proc_root(proc.root.clone());

        assert!(
            monitor
                .scan_at(Duration::from_secs(100))
                .unwrap()
                .is_empty()
        );
        proc.remove(42);

        let finished = monitor.scan_at(Duration::from_secs(101)).unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].name, "make");
        assert_eq!(finished[0].command_line, "make -j8 all");
        assert_eq!(finished[0].duration, Duration::from_secs(101));
    }

    #[test]
    fn detects_zombie_as_finished() {
        let proc = FakeProc::new();
        proc.add(42, "xz", 'S', 0, &["xz", "-9", "file"]);
        let mut monitor = Monitor::with_proc_root(proc.root.clone());

        assert!(monitor.scan_at(Duration::from_secs(10)).unwrap().is_empty());
        proc.add(42, "xz", 'Z', 0, &["xz", "-9", "file"]);

        let finished = monitor.scan_at(Duration::from_secs(70)).unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].name, "xz");
        assert_eq!(finished[0].command_line, "xz -9 file");
    }

    #[test]
    fn ignores_unwatched_commands() {
        let proc = FakeProc::new();
        proc.add(42, "bash", 'S', 0, &["bash", "-c", "sleep 120"]);
        let mut monitor = Monitor::with_proc_root(proc.root.clone());

        assert!(monitor.scan_at(Duration::from_secs(10)).unwrap().is_empty());
        proc.remove(42);

        assert!(monitor.scan_at(Duration::from_secs(70)).unwrap().is_empty());
    }

    #[test]
    fn detects_old_process_when_pid_is_reused() {
        let proc = FakeProc::new();
        proc.add(42, "wget", 'S', 0, &["wget", "old"]);
        let mut monitor = Monitor::with_proc_root(proc.root.clone());

        assert!(monitor.scan_at(Duration::from_secs(10)).unwrap().is_empty());
        proc.add(42, "wget", 'S', 3_000, &["wget", "new"]);

        let finished = monitor.scan_at(Duration::from_secs(70)).unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].command_line, "wget old");
    }
}
