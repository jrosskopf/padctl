use crate::ble::protocol::{MachineStatus, StatusPacket};

/// A BLE device discovered during scanning.
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub name: String,
    pub addr: String, // "AA:BB:CC:DD:EE:FF" from peripheral.address().to_string()
    pub rssi: Option<i16>,
}

/// Events flowing from the BLE task → TUI.
#[derive(Debug)]
#[allow(dead_code)]
pub enum BleEvent {
    Connected,
    Disconnected,
    /// Live telemetry from the FTMS Treadmill Data notification (speed, distance, time).
    Status(StatusPacket),
    /// Step count from the proprietary 0xfff1 notification.
    StepCount(u32),
    /// State-change event from the FTMS Fitness Machine Status characteristic (0x2ADA).
    MachineStatus(MachineStatus),
    /// A device was discovered during BLE scanning.
    DeviceFound(DiscoveredDevice),
    Error(String),
}

/// Commands flowing from the TUI → BLE task.
#[derive(Debug)]
pub enum Cmd {
    StartBelt,
    StopBelt,
    SetSpeed(u8),
    SetMode(u8),
    Quit,
}
