use std::fmt::{Display, Formatter};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitCode {
    ConfigIo = 3,
    ConfigInvalid = 4,
    StartupFailed = 5,
    RuntimeFailed = 6,
    ShutdownTimedOut = 7,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        self as i32
    }
}

#[derive(Debug)]
pub struct AppError {
    exit_code: ExitCode,
    message: String,
}

impl AppError {
    pub fn new(exit_code: ExitCode, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            message: message.into(),
        }
    }

    pub fn config_io(path: &Path, error: impl Display) -> Self {
        Self::new(
            ExitCode::ConfigIo,
            format!(
                "failed to read configuration at {}: {error}",
                path.display()
            ),
        )
    }

    pub fn config_invalid(message: impl Into<String>) -> Self {
        Self::new(ExitCode::ConfigInvalid, message)
    }

    pub fn startup_failed(message: impl Into<String>) -> Self {
        Self::new(ExitCode::StartupFailed, message)
    }

    pub fn runtime_failed(message: impl Into<String>) -> Self {
        Self::new(ExitCode::RuntimeFailed, message)
    }

    pub fn shutdown_timed_out(message: impl Into<String>) -> Self {
        Self::new(ExitCode::ShutdownTimedOut, message)
    }

    pub fn exit_code(&self) -> ExitCode {
        self.exit_code
    }
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

pub type AppResult<T> = Result<T, AppError>;
