use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("BLE error: {0}")]
    Ble(#[from] btleplug::Error),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Device not found (scanned for {0:?})")]
    DeviceNotFound(String),

    #[error("Not connected")]
    NotConnected,

    #[error("Invalid session state transition: {0}")]
    InvalidTransition(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Google Fit error: {0}")]
    Fit(String),
}

pub type Result<T> = std::result::Result<T, AppError>;
