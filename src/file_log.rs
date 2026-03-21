use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{LogOverflowStrategy, ResolvedLogConfig};
use crate::error::{AppError, AppResult};

pub type SharedFileLogSink = Arc<Mutex<FileLogSink>>;

pub struct FileLogSink {
    config: ResolvedLogConfig,
    writer: Option<BufWriter<std::fs::File>>,
    current_size: u64,
    archive_sequence: u64,
}

impl FileLogSink {
    pub fn open_shared(config: ResolvedLogConfig) -> AppResult<SharedFileLogSink> {
        let (writer, current_size) = open_log_writer(&config.file, config.append)?;
        Ok(Arc::new(Mutex::new(Self {
            config,
            writer: Some(writer),
            current_size,
            archive_sequence: 0,
        })))
    }

    pub fn write_line(&mut self, line: &str) -> AppResult<()> {
        let payload = format!("{line}\n");
        let payload_size = payload.len() as u64;

        if let Some(max_file_bytes) = self.config.max_file_bytes
            && self.current_size > 0
            && self.current_size.saturating_add(payload_size) > max_file_bytes
        {
            self.overflow()?;
        }

        let writer = self
            .writer
            .as_mut()
            .expect("file log writer must be available while writing");
        writer.write_all(payload.as_bytes()).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to write log file {}: {error}",
                self.config.file.display()
            ))
        })?;
        writer.flush().map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to flush log file {}: {error}",
                self.config.file.display()
            ))
        })?;
        self.current_size = self.current_size.saturating_add(payload_size);
        Ok(())
    }

    fn overflow(&mut self) -> AppResult<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush().map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to flush log file {} before overflow handling: {error}",
                    self.config.file.display()
                ))
            })?;
        }
        let writer = self.writer.take();
        drop(writer);

        match self.config.overflow_strategy {
            Some(LogOverflowStrategy::Rotate) => self.rotate_files()?,
            Some(LogOverflowStrategy::Archive) => self.archive_file()?,
            None => unreachable!("validated log configuration must set overflow_strategy"),
        }

        let (writer, size) = open_log_writer(&self.config.file, false)?;
        self.writer = Some(writer);
        self.current_size = size;
        Ok(())
    }

    fn rotate_files(&mut self) -> AppResult<()> {
        let rotate_count = self
            .config
            .rotate_file_count
            .expect("validated rotate configuration must include rotate_file_count");

        let oldest = rotate_path(&self.config.file, rotate_count);
        if oldest.exists() {
            fs::remove_file(&oldest).map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to remove rotated log file {}: {error}",
                    oldest.display()
                ))
            })?;
        }

        for index in (1..rotate_count).rev() {
            let source = rotate_path(&self.config.file, index);
            let target = rotate_path(&self.config.file, index + 1);
            if source.exists() {
                fs::rename(&source, &target).map_err(|error| {
                    AppError::runtime_failed(format!(
                        "failed to rename rotated log file {} to {}: {error}",
                        source.display(),
                        target.display()
                    ))
                })?;
            }
        }

        if self.config.file.exists() {
            let first = rotate_path(&self.config.file, 1);
            fs::rename(&self.config.file, &first).map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to rotate active log file {} to {}: {error}",
                    self.config.file.display(),
                    first.display()
                ))
            })?;
        }

        Ok(())
    }

    fn archive_file(&mut self) -> AppResult<()> {
        if !self.config.file.exists() {
            return Ok(());
        }

        self.archive_sequence = self.archive_sequence.saturating_add(1);
        let archive_path = archive_path(&self.config.file, self.archive_sequence);
        fs::rename(&self.config.file, &archive_path).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to archive log file {} to {}: {error}",
                self.config.file.display(),
                archive_path.display()
            ))
        })?;
        Ok(())
    }
}

fn open_log_writer(path: &Path, append: bool) -> AppResult<(BufWriter<std::fs::File>, u64)> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::startup_failed(format!(
                "failed to create log directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let existing_size = if append && path.exists() {
        path.metadata().map(|metadata| metadata.len()).unwrap_or(0)
    } else {
        0
    };

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)
        .map_err(|error| {
            AppError::startup_failed(format!(
                "failed to open log file {}: {error}",
                path.display()
            ))
        })?;

    Ok((BufWriter::new(file), existing_size))
}

fn rotate_path(path: &Path, index: usize) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(format!(".{index}"));
    PathBuf::from(os)
}

fn archive_path(path: &Path, sequence: u64) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after unix epoch")
        .as_millis();
    let mut os = path.as_os_str().to_os_string();
    os.push(format!(".{millis}.{sequence:03}"));
    PathBuf::from(os)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config::{LogOverflowStrategy, ResolvedLogConfig};

    use super::FileLogSink;

    #[test]
    fn rotate_creates_numbered_history_files() {
        let dir = temp_dir("rotate");
        let log_path = dir.join("app.log");
        let sink = FileLogSink::open_shared(ResolvedLogConfig {
            file: log_path.clone(),
            append: true,
            max_file_bytes: Some(25),
            overflow_strategy: Some(LogOverflowStrategy::Rotate),
            rotate_file_count: Some(2),
        })
        .unwrap();

        {
            let mut sink = sink.lock().unwrap();
            sink.write_line("[out] 1234567890").unwrap();
            sink.write_line("[out] abcdefghij").unwrap();
            sink.write_line("[out] klmnopqrst").unwrap();
        }

        assert!(log_path.exists());
        assert!(PathBuf::from(format!("{}.1", log_path.display())).exists());
        assert!(!fs::read_to_string(&log_path).unwrap().is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn archive_creates_additional_files() {
        let dir = temp_dir("archive");
        let log_path = dir.join("worker.log");
        let sink = FileLogSink::open_shared(ResolvedLogConfig {
            file: log_path.clone(),
            append: true,
            max_file_bytes: Some(25),
            overflow_strategy: Some(LogOverflowStrategy::Archive),
            rotate_file_count: None,
        })
        .unwrap();

        {
            let mut sink = sink.lock().unwrap();
            sink.write_line("[out] 1234567890").unwrap();
            sink.write_line("[out] abcdefghij").unwrap();
            sink.write_line("[out] klmnopqrst").unwrap();
        }

        let archived = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("worker.log.")
            })
            .count();
        assert!(archived >= 1);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_false_replaces_existing_file() {
        let dir = temp_dir("truncate");
        let log_path = dir.join("instance.log");
        fs::write(&log_path, "old\n").unwrap();

        let sink = FileLogSink::open_shared(ResolvedLogConfig {
            file: log_path.clone(),
            append: false,
            max_file_bytes: None,
            overflow_strategy: None,
            rotate_file_count: None,
        })
        .unwrap();

        {
            let mut sink = sink.lock().unwrap();
            sink.write_line("new").unwrap();
        }

        let raw = fs::read_to_string(&log_path).unwrap();
        assert_eq!(raw, "new\n");

        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onekey-run-rs-file-log-{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
