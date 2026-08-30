#![allow(dead_code)]

use crate::ble::protocol::StatusPacket;
use crate::error::Result;
use async_trait::async_trait;

/// Abstraction over the BLE hardware layer.
/// Implemented by `BleDriver` (real) and `MockWalkingPadBle` (tests).
#[async_trait]
pub trait WalkingPadBle: Send + Sync {
    /// Connect to the device (scan by name or address).
    async fn connect(&mut self) -> Result<()>;

    /// Disconnect cleanly.
    async fn disconnect(&mut self) -> Result<()>;

    /// Send a raw command packet (variable-length for FTMS).
    async fn write_command(&self, pkt: &[u8]) -> Result<()>;

    /// Poll for the current status packet.
    async fn ask_stats(&self) -> Result<StatusPacket>;

    /// Returns true if BLE connection is active.
    fn is_connected(&self) -> bool;
}
