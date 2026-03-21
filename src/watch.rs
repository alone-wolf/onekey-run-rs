use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchEvent {
    pub service_name: String,
    pub changed_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct WatchRequest {
    pub service_name: String,
    pub paths: Vec<PathBuf>,
    pub ignore_paths: Vec<PathBuf>,
    pub poll_interval: Duration,
}

pub struct WatchHandle {
    stop_flag: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathFingerprint {
    kind: EntryKind,
    len: u64,
    modified_nanos: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    File,
    Dir,
    Symlink,
}

impl WatchHandle {
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

pub fn spawn_service_watcher(request: WatchRequest, sender: Sender<WatchEvent>) -> WatchHandle {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let thread_stop_flag = stop_flag.clone();

    thread::spawn(move || {
        let mut snapshot = capture_snapshot(&request.paths, &request.ignore_paths);

        while !thread_stop_flag.load(Ordering::SeqCst) {
            thread::sleep(request.poll_interval);
            let next_snapshot = capture_snapshot(&request.paths, &request.ignore_paths);
            if let Some(changed_path) = first_changed_path(&snapshot, &next_snapshot) {
                if sender
                    .send(WatchEvent {
                        service_name: request.service_name.clone(),
                        changed_path,
                    })
                    .is_err()
                {
                    break;
                }
            }
            snapshot = next_snapshot;
        }
    });

    WatchHandle { stop_flag }
}

fn capture_snapshot(
    paths: &[PathBuf],
    ignore_paths: &[PathBuf],
) -> BTreeMap<PathBuf, PathFingerprint> {
    let mut snapshot = BTreeMap::new();
    for path in paths {
        collect_path(path, ignore_paths, &mut snapshot);
    }
    snapshot
}

fn collect_path(
    path: &Path,
    ignore_paths: &[PathBuf],
    snapshot: &mut BTreeMap<PathBuf, PathFingerprint>,
) {
    if is_ignored_path(path, ignore_paths) {
        return;
    }

    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };

    let fingerprint = fingerprint_for(&metadata);
    snapshot.insert(path.to_path_buf(), fingerprint);

    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };

        let mut children = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            collect_path(&child, ignore_paths, snapshot);
        }
    }
}

fn fingerprint_for(metadata: &fs::Metadata) -> PathFingerprint {
    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        EntryKind::Dir
    } else if file_type.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };

    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    PathFingerprint {
        kind,
        len: metadata.len(),
        modified_nanos,
    }
}

fn first_changed_path(
    previous: &BTreeMap<PathBuf, PathFingerprint>,
    next: &BTreeMap<PathBuf, PathFingerprint>,
) -> Option<PathBuf> {
    for (path, fingerprint) in previous {
        if next.get(path) != Some(fingerprint) {
            return Some(path.clone());
        }
    }

    for path in next.keys() {
        if !previous.contains_key(path) {
            return Some(path.clone());
        }
    }

    None
}

fn is_ignored_path(path: &Path, ignore_paths: &[PathBuf]) -> bool {
    ignore_paths
        .iter()
        .any(|ignored| path == ignored || path.starts_with(ignored))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{WatchRequest, spawn_service_watcher};

    #[test]
    fn watcher_reports_file_changes() {
        let dir = temp_dir("watch-file");
        let file = dir.join("app.txt");
        fs::write(&file, "v1\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let handle = spawn_service_watcher(
            WatchRequest {
                service_name: "api".to_owned(),
                paths: vec![file.clone()],
                ignore_paths: Vec::new(),
                poll_interval: Duration::from_millis(50),
            },
            tx,
        );

        std::thread::sleep(Duration::from_millis(80));
        fs::write(&file, "v2\n").unwrap();

        let event = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(event.service_name, "api");
        assert_eq!(event.changed_path, file);

        handle.stop();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn watcher_reports_directory_child_changes() {
        let dir = temp_dir("watch-dir");
        let src_dir = dir.join("src");
        let file = src_dir.join("main.rs");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(&file, "fn main() {}\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let handle = spawn_service_watcher(
            WatchRequest {
                service_name: "api".to_owned(),
                paths: vec![src_dir.clone()],
                ignore_paths: Vec::new(),
                poll_interval: Duration::from_millis(50),
            },
            tx,
        );

        std::thread::sleep(Duration::from_millis(80));
        fs::write(&file, "fn main() { println!(\"hi\"); }\n").unwrap();

        let event = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(event.service_name, "api");
        assert!(event.changed_path == file || event.changed_path == src_dir);

        handle.stop();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn watcher_ignores_explicit_paths() {
        let dir = temp_dir("watch-ignore");
        let logs_dir = dir.join("logs");
        let watched_file = dir.join("src.txt");
        let ignored_file = logs_dir.join("app.log");
        fs::create_dir_all(&logs_dir).unwrap();
        fs::write(&watched_file, "v1\n").unwrap();
        fs::write(&ignored_file, "log1\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let handle = spawn_service_watcher(
            WatchRequest {
                service_name: "api".to_owned(),
                paths: vec![dir.clone()],
                ignore_paths: vec![logs_dir.clone()],
                poll_interval: Duration::from_millis(50),
            },
            tx,
        );

        std::thread::sleep(Duration::from_millis(80));
        fs::write(&ignored_file, "log2\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());

        fs::write(&watched_file, "v2\n").unwrap();
        let event = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(event.service_name, "api");
        assert!(event.changed_path == watched_file || event.changed_path == dir);

        handle.stop();
        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onekey-run-rs-watch-{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
