use std::{
    collections::{HashMap, HashSet},
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags};

const IDLE_NOTIFY_DELAY: Duration = Duration::from_secs(30);
const OPENCODE_COMMAND: &str = "opencode";

const STATUS_QUERY: &str = r#"
SELECT
    s.id,
    s.directory,
    s.title,
    json_extract(m.data, '$.role'),
    json_extract(m.data, '$.finish'),
    json_extract(m.data, '$.time.completed'),
    json_extract(m.data, '$.parentID'),
    (
        SELECT mu.id
        FROM message mu
        WHERE mu.session_id = s.id
          AND json_extract(mu.data, '$.role') = 'user'
        ORDER BY mu.time_created DESC, mu.id DESC
        LIMIT 1
    ),
    (
        SELECT COUNT(*)
        FROM part p
        WHERE p.message_id = m.id
          AND json_extract(p.data, '$.type') = 'tool'
          AND COALESCE(
              json_extract(p.data, '$.metadata.providerExecuted'),
              0
          ) = 0
          AND NOT (
              json_extract(p.data, '$.state.status') = 'error'
              AND COALESCE(
                  json_extract(p.data, '$.state.metadata.interrupted'),
                  0
              ) = 1
          )
    )
FROM session s
JOIN message m ON m.id = (
    SELECT m2.id
    FROM message m2
    WHERE m2.session_id = s.id
    ORDER BY m2.time_created DESC, m2.id DESC
    LIMIT 1
)
WHERE s.parent_id IS NULL
  AND s.time_archived IS NULL
"#;

#[derive(Debug)]
pub struct ReadySession {
    pub title: String,
    pub directory: String,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Busy,
    Idle { since: Instant, notified: bool },
}

struct SessionStatus {
    id: String,
    title: String,
    directory: String,
    idle: bool,
}

pub struct Watcher {
    db_path: PathBuf,
    proc_root: Option<PathBuf>,
    connection: Option<Connection>,
    states: HashMap<String, State>,
}

impl Watcher {
    pub fn new() -> Self {
        Self::for_database(discover_database(), Some(PathBuf::from("/proc")))
    }

    fn for_database(db_path: PathBuf, proc_root: Option<PathBuf>) -> Self {
        Self {
            db_path,
            proc_root,
            connection: None,
            states: HashMap::new(),
        }
    }

    #[cfg(test)]
    fn for_test(db_path: PathBuf) -> Self {
        Self::for_database(db_path, None)
    }

    #[cfg(test)]
    fn for_test_with_proc(db_path: PathBuf, proc_root: PathBuf) -> Self {
        Self::for_database(db_path, Some(proc_root))
    }

    pub fn scan(&mut self) -> io::Result<Vec<ReadySession>> {
        self.scan_at(Instant::now())
    }

    fn scan_at(&mut self, now: Instant) -> io::Result<Vec<ReadySession>> {
        if !self.db_path.is_file() {
            return Ok(Vec::new());
        }

        let statuses = {
            let connection = self.connection()?;
            read_statuses(connection)?
        };

        let running = match &self.proc_root {
            Some(proc_root) => Some(running_opencode_directories(proc_root)?),
            None => None,
        };

        let mut ready = Vec::new();
        let mut active = HashSet::new();
        for status in statuses {
            active.insert(status.id.clone());
            let next = match self.states.get(&status.id) {
                None if status.idle => State::Idle {
                    since: now,
                    notified: true,
                },
                None => State::Busy,
                Some(State::Busy) if status.idle => State::Idle {
                    since: now,
                    notified: false,
                },
                Some(State::Busy) => State::Busy,
                Some(State::Idle {
                    since,
                    notified: false,
                }) if status.idle
                    && now.saturating_duration_since(*since)
                        > IDLE_NOTIFY_DELAY =>
                {
                    ready.push(ReadySession {
                        title: status.title,
                        directory: status.directory,
                    });
                    State::Idle {
                        since: *since,
                        notified: true,
                    }
                }
                Some(State::Idle { since, notified }) if status.idle => {
                    State::Idle {
                        since: *since,
                        notified: *notified,
                    }
                }
                Some(State::Idle { .. }) => State::Busy,
            };
            self.states.insert(status.id, next);
        }
        self.states.retain(|id, _| active.contains(id));

        if let Some(running) = running {
            ready.retain(|session| {
                running.contains(&directory_key(Path::new(&session.directory)))
            });
        }
        Ok(ready)
    }

    fn connection(&mut self) -> io::Result<&Connection> {
        if self.connection.is_none() {
            let connection = Connection::open_with_flags(
                &self.db_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(io::Error::other)?;
            connection
                .busy_timeout(Duration::from_millis(1_000))
                .map_err(io::Error::other)?;
            self.connection = Some(connection);
        }
        self.connection
            .as_ref()
            .ok_or_else(|| io::Error::other("database connection unavailable"))
    }
}

fn running_opencode_directories(
    proc_root: &Path,
) -> io::Result<HashSet<PathBuf>> {
    let mut directories = HashSet::new();
    for entry in fs::read_dir(proc_root)? {
        let Ok(entry) = entry else {
            continue;
        };
        let Some(pid) = entry.file_name().to_string_lossy().parse::<u32>().ok()
        else {
            continue;
        };
        let path = proc_root.join(pid.to_string());
        if !is_live_opencode(&path) {
            continue;
        }
        if let Some(directory) = process_directory(&path) {
            directories.insert(directory_key(&directory));
        }
    }
    Ok(directories)
}

fn is_live_opencode(proc_path: &Path) -> bool {
    let Ok(data) = fs::read_to_string(proc_path.join("stat")) else {
        return false;
    };
    let Some((comm, state)) = parse_stat(&data) else {
        return false;
    };
    comm == OPENCODE_COMMAND && !matches!(state, 'Z' | 'X' | 'x')
}

fn parse_stat(data: &str) -> Option<(&str, char)> {
    let open = data.find('(')?;
    let close = data.rfind(')')?;
    if close <= open {
        return None;
    }
    let comm = data.get(open + 1..close)?;
    let state = data
        .get(close + 1..)?
        .split_whitespace()
        .next()?
        .chars()
        .next()?;
    Some((comm, state))
}

fn process_directory(proc_path: &Path) -> Option<PathBuf> {
    let cwd = fs::read_link(proc_path.join("cwd")).ok()?;
    let cwd = cwd.to_string_lossy();
    let cwd = cwd.strip_suffix(" (deleted)").unwrap_or(&cwd);
    Some(PathBuf::from(cwd))
}

fn directory_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn read_statuses(connection: &Connection) -> io::Result<Vec<SessionStatus>> {
    let mut statement = connection
        .prepare_cached(STATUS_QUERY)
        .map_err(io::Error::other)?;
    let rows = statement
        .query_map([], |row| {
            let role: Option<String> = row.get(3)?;
            let finish: Option<String> = row.get(4)?;
            let completed: Option<i64> = row.get(5)?;
            let parent_id: Option<String> = row.get(6)?;
            let latest_user_id: Option<String> = row.get(7)?;
            let pending_tools: i64 = row.get(8)?;

            let idle = role.as_deref() == Some("assistant")
                && completed.is_some()
                && finish.as_deref().is_some_and(|finish| {
                    finish != "tool-calls" && finish != "unknown"
                })
                && parent_id.is_some()
                && parent_id == latest_user_id
                && pending_tools == 0;

            Ok(SessionStatus {
                id: row.get(0)?,
                directory: row.get(1)?,
                title: row.get(2)?,
                idle,
            })
        })
        .map_err(io::Error::other)?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)
}

fn discover_database() -> PathBuf {
    opencode_database_path()
        .or_else(default_database_path)
        .unwrap_or_else(|| PathBuf::from("opencode.db"))
}

fn opencode_database_path() -> Option<PathBuf> {
    let output = Command::new("opencode")
        .args(["db", "path"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn default_database_path() -> Option<PathBuf> {
    let data_home =
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share"))
            })?;
    if let Some(database) = env::var_os("OPENCODE_DB") {
        let database = PathBuf::from(database);
        if database.is_absolute() {
            return Some(database);
        }
        if database == Path::new(":memory:") {
            return None;
        }
        return Some(data_home.join(database));
    }
    Some(data_home.join("opencode").join("opencode.db"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU32, Ordering},
    };

    use super::*;

    static NEXT_ID: AtomicU32 = AtomicU32::new(0);

    struct TestDb {
        path: PathBuf,
        connection: Connection,
    }

    impl TestDb {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "wakeup-opencode-{}-{id}.db",
                std::process::id()
            ));
            let _ = fs::remove_file(&path);
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "
                    CREATE TABLE session (
                        id TEXT PRIMARY KEY,
                        parent_id TEXT,
                        directory TEXT NOT NULL,
                        title TEXT NOT NULL,
                        time_archived INTEGER
                    );
                    CREATE TABLE message (
                        id TEXT PRIMARY KEY,
                        session_id TEXT NOT NULL,
                        time_created INTEGER NOT NULL,
                        data TEXT NOT NULL
                    );
                    CREATE TABLE part (
                        id TEXT PRIMARY KEY,
                        message_id TEXT NOT NULL,
                        session_id TEXT NOT NULL,
                        time_created INTEGER NOT NULL,
                        data TEXT NOT NULL
                    );
                    ",
                )
                .unwrap();
            Self { path, connection }
        }

        fn session(
            &self,
            id: &str,
            parent_id: Option<&str>,
            directory: &str,
            title: &str,
        ) {
            self.connection
                .execute(
                    "INSERT INTO session
                     (id, parent_id, directory, title, time_archived)
                     VALUES (?1, ?2, ?3, ?4, NULL)",
                    (id, parent_id, directory, title),
                )
                .unwrap();
        }

        fn message(&self, id: &str, session: &str, created: i64, data: &str) {
            self.connection
                .execute(
                    "INSERT INTO message
                     (id, session_id, time_created, data)
                     VALUES (?1, ?2, ?3, ?4)",
                    (id, session, created, data),
                )
                .unwrap();
        }

        fn update_message(&self, id: &str, data: &str) {
            self.connection
                .execute(
                    "UPDATE message SET data = ?1 WHERE id = ?2",
                    (data, id),
                )
                .unwrap();
        }

        fn tool_part(&self, id: &str, message: &str, state: &str) {
            let data =
                format!(r#"{{"type":"tool","state":{{"status":"{state}"}}}}"#);
            self.connection
                .execute(
                    "INSERT INTO part
                     (id, message_id, session_id, time_created, data)
                     VALUES (?1, ?2, 'root', 1, ?3)",
                    (id, message, data),
                )
                .unwrap();
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let wal = format!("{}-wal", self.path.display());
            let shm = format!("{}-shm", self.path.display());
            let _ = fs::remove_file(wal);
            let _ = fs::remove_file(shm);
        }
    }

    struct FakeProc {
        root: PathBuf,
    }

    impl FakeProc {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = env::temp_dir()
                .join(format!("wakeup-proc-{}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn add(&self, pid: u32, comm: &str, state: char, cwd: &Path) {
            let dir = self.root.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("stat"),
                format!("{pid} ({comm}) {state} 1 1 1"),
            )
            .unwrap();
            std::os::unix::fs::symlink(cwd, dir.join("cwd")).unwrap();
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

    fn user_message() -> &'static str {
        r#"{"role":"user"}"#
    }

    fn assistant_message(
        parent: &str,
        finish: Option<&str>,
        completed: Option<i64>,
    ) -> String {
        let finish = finish
            .map(|finish| format!(r#""{finish}""#))
            .unwrap_or_else(|| "null".to_owned());
        let completed = completed
            .map(|completed| completed.to_string())
            .unwrap_or_else(|| "null".to_owned());
        format!(
            r#"{{"role":"assistant","parentID":"{parent}","finish":{finish},"time":{{"created":1,"completed":{completed}}}}}"#
        )
    }

    #[test]
    fn notifies_after_waiting_over_thirty_seconds() {
        let db = TestDb::new();
        db.session("root", None, "/tmp/project", "Fix the bug");
        db.message("user", "root", 1, user_message());
        db.message(
            "assistant",
            "root",
            2,
            &assistant_message("user", None, None),
        );
        let mut watcher = Watcher::for_test(db.path.clone());
        let start = Instant::now();

        assert!(watcher.scan_at(start).unwrap().is_empty());
        db.update_message(
            "assistant",
            &assistant_message("user", Some("stop"), Some(3)),
        );

        assert!(
            watcher
                .scan_at(start + Duration::from_secs(5))
                .unwrap()
                .is_empty()
        );
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(34))
                .unwrap()
                .is_empty()
        );

        let ready = watcher.scan_at(start + Duration::from_secs(36)).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].title, "Fix the bug");
        assert_eq!(ready[0].directory, "/tmp/project");
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(100))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn finds_directories_of_running_opencode_instances() {
        let proc = FakeProc::new();
        let project = proc.root.join("project");
        fs::create_dir_all(&project).unwrap();
        proc.add(10, "opencode", 'S', &project);
        proc.add(11, "opencode", 'Z', &project);
        proc.add(12, "bash", 'S', &project);

        let directories = running_opencode_directories(&proc.root).unwrap();

        assert_eq!(directories, HashSet::from([directory_key(&project)]));
    }

    #[test]
    fn does_not_notify_when_opencode_instance_quit() {
        let proc = FakeProc::new();
        let project = proc.root.join("project");
        fs::create_dir_all(&project).unwrap();
        proc.add(10, "opencode", 'S', &project);

        let db = TestDb::new();
        db.session("root", None, project.to_str().unwrap(), "Fix the bug");
        db.message("user", "root", 1, user_message());
        db.message(
            "assistant",
            "root",
            2,
            &assistant_message("user", None, None),
        );
        let mut watcher =
            Watcher::for_test_with_proc(db.path.clone(), proc.root.clone());
        let start = Instant::now();

        assert!(watcher.scan_at(start).unwrap().is_empty());
        db.update_message(
            "assistant",
            &assistant_message("user", Some("stop"), Some(3)),
        );
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(5))
                .unwrap()
                .is_empty()
        );

        proc.remove(10);

        assert!(
            watcher
                .scan_at(start + Duration::from_secs(36))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn notifies_when_opencode_instance_is_still_running() {
        let proc = FakeProc::new();
        let project = proc.root.join("project");
        fs::create_dir_all(&project).unwrap();
        proc.add(10, "opencode", 'S', &project);

        let db = TestDb::new();
        db.session("root", None, project.to_str().unwrap(), "Fix the bug");
        db.message("user", "root", 1, user_message());
        db.message(
            "assistant",
            "root",
            2,
            &assistant_message("user", None, None),
        );
        let mut watcher =
            Watcher::for_test_with_proc(db.path.clone(), proc.root.clone());
        let start = Instant::now();

        assert!(watcher.scan_at(start).unwrap().is_empty());
        db.update_message(
            "assistant",
            &assistant_message("user", Some("stop"), Some(3)),
        );
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(5))
                .unwrap()
                .is_empty()
        );

        let ready = watcher.scan_at(start + Duration::from_secs(36)).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].title, "Fix the bug");
    }

    #[test]
    fn does_not_notify_if_user_returns_before_grace_period() {
        let db = TestDb::new();
        db.session("root", None, "/tmp/project", "Fix the bug");
        db.message("user", "root", 1, user_message());
        db.message(
            "assistant",
            "root",
            2,
            &assistant_message("user", None, None),
        );
        let mut watcher = Watcher::for_test(db.path.clone());
        let start = Instant::now();

        assert!(watcher.scan_at(start).unwrap().is_empty());
        db.update_message(
            "assistant",
            &assistant_message("user", Some("stop"), Some(3)),
        );
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(5))
                .unwrap()
                .is_empty()
        );

        db.message("user-2", "root", 4, user_message());
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(20))
                .unwrap()
                .is_empty()
        );
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(40))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn initially_idle_session_is_not_notified() {
        let db = TestDb::new();
        db.session("root", None, "/tmp/project", "Already done");
        db.message("user", "root", 1, user_message());
        db.message(
            "assistant",
            "root",
            2,
            &assistant_message("user", Some("stop"), Some(3)),
        );
        let mut watcher = Watcher::for_test(db.path.clone());
        let start = Instant::now();

        assert!(watcher.scan_at(start).unwrap().is_empty());
        assert!(
            watcher
                .scan_at(start + Duration::from_secs(31))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ignores_subagent_sessions() {
        let db = TestDb::new();
        db.session("child", Some("parent"), "/tmp/project", "Subagent");
        db.message("user", "child", 1, user_message());
        db.message(
            "assistant",
            "child",
            2,
            &assistant_message("user", None, None),
        );
        let mut watcher = Watcher::for_test(db.path.clone());
        let start = Instant::now();

        assert!(watcher.scan_at(start).unwrap().is_empty());
        db.update_message(
            "assistant",
            &assistant_message("user", Some("stop"), Some(3)),
        );

        assert!(
            watcher
                .scan_at(start + Duration::from_secs(31))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn waits_for_tool_calls_to_finish() {
        let db = TestDb::new();
        db.session("root", None, "/tmp/project", "Run tests");
        db.message("user", "root", 1, user_message());
        db.message(
            "assistant",
            "root",
            2,
            &assistant_message("user", Some("stop"), Some(3)),
        );
        db.tool_part("tool", "assistant", "completed");
        let mut watcher = Watcher::for_test(db.path.clone());
        let start = Instant::now();

        assert!(watcher.scan_at(start).unwrap().is_empty());
        db.connection
            .execute("DELETE FROM part WHERE id = 'tool'", [])
            .unwrap();

        assert!(
            watcher
                .scan_at(start + Duration::from_secs(5))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            watcher
                .scan_at(start + Duration::from_secs(36))
                .unwrap()
                .len(),
            1
        );
    }
}
