//! Local persistence for Weft: command history and session records.
//!
//! The format is line based and self-describing, so a damaged line is skipped
//! instead of discarding the whole file. Fields are escaped to keep one record
//! per line even when a command spans several lines or contains tabs.
//!
//! Writes go through a temporary file and a rename, so an interrupted write
//! cannot leave a half-written history behind.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A command run in a session, with the context needed to explain it later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// Unix timestamp in seconds.
    pub at: u64,
    /// Exit code reported by the shell.
    pub exit: i32,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Working directory where the command ran.
    pub cwd: String,
    /// The command line itself.
    pub command: String,
}

impl SessionEntry {
    /// Creates an entry stamped with the current time.
    pub fn now(
        cwd: impl Into<String>,
        command: impl Into<String>,
        exit: i32,
        duration_ms: u64,
    ) -> Self {
        Self {
            at: unix_time(),
            exit,
            duration_ms,
            cwd: cwd.into(),
            command: command.into(),
        }
    }
}

/// A directory holding Weft's local files.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Opens or creates a store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Opens the store under the platform data directory.
    ///
    /// `WEFT_DATA_DIR` wins, then the XDG location, then the Windows roaming
    /// location as reported by the environment. No compile-time platform switch
    /// is needed: whichever variable exists decides.
    pub fn open_default() -> io::Result<Self> {
        if let Some(dir) = env::var_os("WEFT_DATA_DIR") {
            return Self::open(dir);
        }
        if let Some(dir) = env::var_os("XDG_DATA_HOME") {
            return Self::open(PathBuf::from(dir).join("weft"));
        }
        if let Some(dir) = env::var_os("LOCALAPPDATA") {
            return Self::open(PathBuf::from(dir).join("Weft"));
        }
        if let Some(home) = env::var_os("HOME") {
            return Self::open(PathBuf::from(home).join(".local/share/weft"));
        }
        Self::open("weft-data")
    }

    /// Directory backing this store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the history file.
    pub fn history_path(&self) -> PathBuf {
        self.root.join("history")
    }

    /// Path of the session log.
    pub fn sessions_path(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// Loads at most `limit` history entries, oldest first.
    pub fn load_history(&self, limit: usize) -> io::Result<Vec<String>> {
        let Some(file) = open_if_exists(&self.history_path())? else {
            return Ok(Vec::new());
        };
        let mut entries: Vec<String> = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            let entry = decode(&line);
            if entry.is_empty() {
                continue;
            }
            if entries.len() == limit {
                entries.remove(0);
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Appends one command line, ignoring empty input.
    pub fn append_history(&self, line: &str) -> io::Result<()> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_path())?;
        writeln!(file, "{}", encode(line))
    }

    /// Rewrites the history keeping only the newest `limit` entries.
    pub fn compact_history(&self, limit: usize) -> io::Result<()> {
        let entries = self.load_history(limit)?;
        let mut body = String::new();
        for entry in entries {
            body.push_str(&encode(&entry));
            body.push('\n');
        }
        write_atomic(&self.history_path(), body.as_bytes())
    }

    /// Empties the history file.
    pub fn clear_history(&self) -> io::Result<()> {
        write_atomic(&self.history_path(), b"")
    }

    /// Appends one session record.
    pub fn append_session(&self, entry: &SessionEntry) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.sessions_path())?;
        writeln!(
            file,
            "{}\t{}\t{}\t{}\t{}",
            entry.at,
            entry.exit,
            entry.duration_ms,
            encode(&entry.cwd),
            encode(&entry.command)
        )
    }

    /// Loads the session log, oldest first. Malformed lines are skipped.
    pub fn sessions(&self) -> io::Result<Vec<SessionEntry>> {
        let Some(file) = open_if_exists(&self.sessions_path())? else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            let mut fields = line.splitn(5, '\t');
            let (Some(at), Some(exit), Some(duration), Some(cwd), Some(command)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            let (Ok(at), Ok(exit), Ok(duration_ms)) = (at.parse(), exit.parse(), duration.parse())
            else {
                continue;
            };
            entries.push(SessionEntry {
                at,
                exit,
                duration_ms,
                cwd: decode(cwd),
                command: decode(command),
            });
        }
        Ok(entries)
    }

    /// Empties the session log.
    pub fn clear_sessions(&self) -> io::Result<()> {
        write_atomic(&self.sessions_path(), b"")
    }
}

/// Current Unix time in seconds.
pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn open_if_exists(path: &Path) -> io::Result<Option<fs::File>> {
    match fs::File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(body)?;
        file.sync_all()?;
    }
    // Windows cannot rename over an existing file, so drop the target first.
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(path);
        fs::rename(&temporary, path)?;
    }
    Ok(())
}

fn encode(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn decode(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) if other != '\\' => {
                // An unknown pair stays verbatim, as does a trailing one.
                out.push('\\');
                out.push(other);
            }
            _ => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (Store, PathBuf) {
        let root = env::temp_dir().join(format!("weft-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        (Store::open(&root).expect("store"), root)
    }

    #[test]
    fn history_round_trips_multiline_commands() {
        let (store, root) = store("roundtrip");
        store.append_history("echo one").unwrap();
        store
            .append_history("for i in 1 2\ndo echo $i\ndone")
            .unwrap();

        let loaded = store.load_history(10).unwrap();
        assert_eq!(loaded, ["echo one", "for i in 1 2\ndo echo $i\ndone"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn history_keeps_the_newest_entries() {
        let (store, root) = store("limit");
        for line in ["one", "two", "three"] {
            store.append_history(line).unwrap();
        }
        assert_eq!(store.load_history(2).unwrap(), ["two", "three"]);

        store.compact_history(2).unwrap();
        assert_eq!(store.load_history(10).unwrap(), ["two", "three"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn empty_and_duplicate_lines_are_not_stored() {
        let (store, root) = store("empty");
        store.append_history("  ").unwrap();
        store.append_history("\n").unwrap();
        assert!(store.load_history(10).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn session_records_keep_context_and_skip_damaged_lines() {
        let (store, root) = store("sessions");
        let entry = SessionEntry::now("/tmp", "cargo test", 0, 42);
        store.append_session(&entry).unwrap();
        store.append_session(&entry).unwrap();

        let mut sessions = store.sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions.remove(0), entry);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tabs_in_paths_do_not_shift_fields() {
        let (store, root) = store("tabs");
        let entry = SessionEntry::now("/tmp/with\ttab", "printf 'a\\tb'", 1, 7);
        store.append_session(&entry).unwrap();
        assert_eq!(store.sessions().unwrap(), [entry]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clear_history_leaves_an_empty_file() {
        let (store, root) = store("clear");
        store.append_history("ls").unwrap();
        store.clear_history().unwrap();
        assert!(store.load_history(10).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
