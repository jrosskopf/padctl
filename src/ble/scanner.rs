use async_trait::async_trait;
use btleplug::api::{
    Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter, ValueNotification, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::{Stream, StreamExt};
use log::{debug, error, warn};
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time;
use uuid::Uuid;

use crate::ble::protocol::{self, StatusPacket};
use crate::ble::traits::WalkingPadBle;
use crate::error::{AppError, Result};
use crate::types::{BleEvent, DiscoveredDevice};

// KS-HD-Z1D implements the standard Bluetooth FTMS (Fitness Machine Service).
// Treadmill Data (notify) and Control Point (write + indicate).
const NOTIFY_UUID: &str = "00002acd-0000-1000-8000-00805f9b34fb"; // Treadmill Data
const WRITE_UUID: &str = "00002ad9-0000-1000-8000-00805f9b34fb"; // Control Point

/// FTMS primary service UUID (0x1826).
///
/// Used as a scan filter so that on macOS (CoreBluetooth) the adapter returns the
/// device even when `local_name` is absent from the advertisement — which is common
/// on macOS due to privacy/power policies. Harmless and slightly more efficient on
/// Linux (BlueZ honours service-UUID filters too).
const FTMS_SERVICE_UUID: &str = "00001826-0000-1000-8000-00805f9b34fb";

/// UUID of the Treadmill Data characteristic (FTMS 0x2ACD).
/// Used by the notification forwarder in main.rs to route FTMS notifications.
pub const TREADMILL_DATA_UUID: &str = NOTIFY_UUID;

/// UUID of the proprietary KingSmith notify characteristic (0xfff1).
/// Sends F7 A2 status packets that include step count — not available in FTMS.
pub const PROPRIETARY_NOTIFY_UUID: &str = "0000fff1-0000-1000-8000-00805f9b34fb";

/// UUID of the proprietary KingSmith write characteristic (0xfff2, WRITE_WITHOUT_RESPONSE).
/// Writing cmd_ask_stats() here triggers a status response on 0xfff1.
const PROPRIETARY_WRITE_UUID: &str = "0000fff2-0000-1000-8000-00805f9b34fb";

/// UUID of the FTMS Fitness Machine Status characteristic (0x2ADA).
/// Pushes state-change events (belt stopped by safety key, started from device panel, etc.)
pub const FITNESS_MACHINE_STATUS_UUID: &str = "00002ada-0000-1000-8000-00805f9b34fb";

// ─── Scanning function (phase 1) ─────────────────────────────────────────────

/// Scan for BLE devices and stream them to `tx`.
///
/// Stops when `stop_rx` receives `true`, then calls `central.stop_scan()` cleanly.
/// Should be spawned as a background task.
pub async fn scan_devices(
    tx: mpsc::Sender<BleEvent>,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<()> {
    debug!("scan_devices: starting");
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Protocol("No BLE adapter found".into()))?;
    debug!("scan_devices: adapter acquired, starting scan");

    let mut events = central.events().await?;
    central
        .start_scan(ScanFilter {
            services: vec![Uuid::parse_str(FTMS_SERVICE_UUID).unwrap()],
        })
        .await?;
    debug!("scan_devices: scan started");

    loop {
        tokio::select! {
            Some(event) = events.next() => {
                let id = match &event {
                    CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => id.clone(),
                    _ => continue,
                };
                if let Ok(peripheral) = central.peripheral(&id).await {
                    let props = peripheral.properties().await.ok().flatten().unwrap_or_default();
                    let addr = peripheral.address().to_string();
                    let name = props.local_name.clone().unwrap_or_else(|| "Unknown".into());
                    let rssi = props.rssi;
                    let adv_services: Vec<_> = props.services.iter().map(|u| u.to_string()).collect();
                    debug!("scan_devices: found device name={name:?} addr={addr} rssi={rssi:?} advertised_services={adv_services:?}");
                    let _ = tx.send(BleEvent::DeviceFound(DiscoveredDevice { name, addr, rssi })).await;
                }
            }
            Ok(()) = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    debug!("scan_devices: stop signal received");
                    break;
                }
            }
        }
    }

    central.stop_scan().await.ok();
    debug!("scan_devices: scan stopped, task exiting");
    Ok(())
}

// ─── BLE session driver (phase 2) ────────────────────────────────────────────

pub struct BleDriver {
    device_name: String,
    device_addr: Option<String>,
    scan_timeout_secs: u64,
    peripheral: Option<Peripheral>,
    write_char: Option<btleplug::api::Characteristic>,
    /// Proprietary write characteristic (0xfff2). Writing cmd_ask_stats() here
    /// triggers a status response on 0xfff1 that includes step count.
    prop_write_char: Option<btleplug::api::Characteristic>,
}

impl BleDriver {
    pub fn new(device_name: String, device_addr: Option<String>, scan_timeout_secs: u64) -> Self {
        Self {
            device_name,
            device_addr,
            scan_timeout_secs,
            peripheral: None,
            write_char: None,
            prop_write_char: None,
        }
    }

    /// Write cmd_ask_stats() to the proprietary 0xfff2 characteristic, which
    /// triggers a status response on 0xfff1 containing step count.
    /// No-op (silently) if 0xfff2 was not found during GATT discovery.
    pub async fn poll_proprietary_stats(&self) -> Result<()> {
        let Some(ref pwc) = self.prop_write_char else {
            return Ok(());
        };
        let p = self.peripheral.as_ref().ok_or(AppError::NotConnected)?;
        debug!("poll_proprietary_stats: writing ask_stats to 0xfff2");
        p.write(
            pwc,
            &crate::ble::protocol::cmd_ask_stats(),
            WriteType::WithoutResponse,
        )
        .await?;
        Ok(())
    }

    /// Perform GATT discovery, characteristic lookup, and subscription on an
    /// already-physically-connected peripheral.
    /// Returns `(ftms_write_char, prop_write_char)` on success; `prop_write_char`
    /// is `None` if 0xfff2 is not present.
    /// Does NOT disconnect on failure — the caller is responsible.
    async fn setup_gatt(
        peripheral: &Peripheral,
    ) -> Result<(
        btleplug::api::Characteristic,
        Option<btleplug::api::Characteristic>,
    )> {
        // Give the device a moment to settle before GATT discovery.
        debug!("setup_gatt: sleeping 400 ms for device settle…");
        time::sleep(Duration::from_millis(400)).await;
        debug!("setup_gatt: settle sleep done");

        // GATT service discovery — can hang indefinitely on BlueZ; cap it.
        debug!("setup_gatt: starting discover_services (10 s timeout)…");
        time::timeout(Duration::from_secs(10), peripheral.discover_services())
            .await
            .map_err(|_| {
                error!("setup_gatt: discover_services timed out after 10 s");
                AppError::Protocol(
                    "GATT service discovery timed out (10 s). \
                     Try power-cycling the WalkingPad."
                        .into(),
                )
            })??;
        debug!("setup_gatt: discover_services completed");

        let chars = peripheral.characteristics();
        debug!("setup_gatt: {} characteristics found:", chars.len());
        for c in &chars {
            debug!(
                "  service={} char={} props={:?}",
                c.service_uuid, c.uuid, c.properties
            );
        }

        if chars.is_empty() {
            error!("setup_gatt: no GATT characteristics found after discovery");
            return Err(AppError::Protocol(
                "No GATT characteristics found after discovery".into(),
            ));
        }

        // Unique services discovered
        let mut seen_services = std::collections::BTreeSet::new();
        for c in &chars {
            seen_services.insert(c.service_uuid);
        }
        debug!(
            "setup_gatt: distinct services: {:?}",
            seen_services
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
        );

        let write_uuid = Uuid::parse_str(WRITE_UUID).unwrap();
        let notify_uuid = Uuid::parse_str(NOTIFY_UUID).unwrap();
        debug!("setup_gatt: looking for write UUID={WRITE_UUID} notify UUID={NOTIFY_UUID}");

        let write_char = chars
            .iter()
            .find(|c| c.uuid == write_uuid)
            .cloned()
            .ok_or_else(|| {
                let details: Vec<_> = chars
                    .iter()
                    .map(|c| format!("svc={} char={}", c.service_uuid, c.uuid))
                    .collect();
                error!("setup_gatt: write characteristic {WRITE_UUID} not found.");
                for d in &details {
                    error!("  {d}");
                }
                AppError::Protocol(format!(
                    "Write characteristic {} not found. Services/chars: {}",
                    WRITE_UUID,
                    details.join(" | ")
                ))
            })?;
        debug!(
            "setup_gatt: write characteristic found: {}",
            write_char.uuid
        );

        // Subscribe to Treadmill Data notifications (0x2ACD)
        if let Some(nc) = chars.iter().find(|c| c.uuid == notify_uuid) {
            debug!("setup_gatt: subscribing to treadmill data char {}", nc.uuid);
            peripheral.subscribe(nc).await?;
            debug!("setup_gatt: subscribed to treadmill data notifications");
        } else {
            warn!("setup_gatt: treadmill data char {NOTIFY_UUID} not found — status updates will not arrive");
        }

        // Subscribe to FTMS Fitness Machine Status (0x2ADA) for device-initiated events (non-fatal).
        let fms_uuid = Uuid::parse_str(FITNESS_MACHINE_STATUS_UUID).unwrap();
        if let Some(fc) = chars.iter().find(|c| c.uuid == fms_uuid) {
            debug!(
                "setup_gatt: subscribing to fitness machine status char {}",
                fc.uuid
            );
            if let Err(e) = peripheral.subscribe(fc).await {
                warn!("setup_gatt: fitness machine status subscribe failed (non-fatal): {e}");
            } else {
                debug!("setup_gatt: subscribed to fitness machine status");
            }
        } else {
            debug!(
                "setup_gatt: fitness machine status char {FITNESS_MACHINE_STATUS_UUID} not found"
            );
        }

        // Subscribe to proprietary 0xfff1 notify char for step count (non-fatal).
        let prop_uuid = Uuid::parse_str(PROPRIETARY_NOTIFY_UUID).unwrap();
        if let Some(pc) = chars.iter().find(|c| c.uuid == prop_uuid) {
            debug!(
                "setup_gatt: subscribing to proprietary char {} for step count",
                pc.uuid
            );
            if let Err(e) = peripheral.subscribe(pc).await {
                warn!("setup_gatt: proprietary notify subscribe failed (non-fatal): {e}");
            } else {
                debug!("setup_gatt: subscribed to proprietary notifications");
            }
        } else {
            debug!("setup_gatt: proprietary char {PROPRIETARY_NOTIFY_UUID} not found — steps unavailable");
        }

        // Subscribe to Control Point indications (0x2AD9) so FTMS responses arrive
        // through the notifications stream. This must happen before RequestControl.
        debug!(
            "setup_gatt: subscribing to control point char {} for FTMS indications",
            write_char.uuid
        );
        if let Err(e) = peripheral.subscribe(&write_char).await {
            warn!("setup_gatt: control point subscribe failed (non-fatal): {e}");
        } else {
            debug!("setup_gatt: control point subscribed");
        }

        // Store proprietary write characteristic (0xfff2) for step-count polling.
        let prop_write_uuid = Uuid::parse_str(PROPRIETARY_WRITE_UUID).unwrap();
        let prop_write_char = chars.iter().find(|c| c.uuid == prop_write_uuid).cloned();
        if prop_write_char.is_some() {
            debug!("setup_gatt: proprietary write char 0xfff2 found — step polling enabled");
        } else {
            debug!(
                "setup_gatt: proprietary write char 0xfff2 not found — steps may be unavailable"
            );
        }

        // FTMS Request Control — must be sent before any command.
        // The indication response arrives via the notification stream once
        // the forwarder task is running; we don't wait for it here.
        debug!("setup_gatt: sending FTMS RequestControl [0x00]…");
        if let Err(e) = peripheral
            .write(&write_char, &[0x00u8], WriteType::WithResponse)
            .await
        {
            warn!("setup_gatt: FTMS RequestControl failed (non-fatal): {e}");
        } else {
            debug!("setup_gatt: FTMS RequestControl sent");
            time::sleep(Duration::from_millis(200)).await;
        }

        Ok((write_char, prop_write_char))
    }

    /// Return the raw notification stream from the connected peripheral.
    /// Call after `connect()`.
    pub async fn get_notifications(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = ValueNotification> + Send>>> {
        debug!("get_notifications: requesting notification stream");
        let p = self.peripheral.as_ref().ok_or(AppError::NotConnected)?;
        let stream = p.notifications().await?;
        debug!("get_notifications: stream acquired");
        Ok(stream)
    }
}

#[async_trait]
impl WalkingPadBle for BleDriver {
    async fn connect(&mut self) -> Result<()> {
        debug!(
            "connect: begin — device_name={:?} device_addr={:?} timeout={}s",
            self.device_name, self.device_addr, self.scan_timeout_secs
        );

        let manager = Manager::new().await?;
        debug!("connect: Manager created");

        let adapters = manager.adapters().await?;
        debug!("connect: {} adapter(s) found", adapters.len());

        let central = adapters
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Protocol("No BLE adapter found".into()))?;
        debug!("connect: adapter selected");

        let mut events = central.events().await?;
        central
            .start_scan(ScanFilter {
                services: vec![Uuid::parse_str(FTMS_SERVICE_UUID).unwrap()],
            })
            .await?;
        debug!("connect: BLE scan started, waiting for device…");

        let scan_timeout = time::sleep(Duration::from_secs(self.scan_timeout_secs));
        tokio::pin!(scan_timeout);

        // Find the device by addr (preferred) or name, using the event stream
        // so we stop scanning the instant we find a match.
        let peripheral = loop {
            tokio::select! {
                _ = &mut scan_timeout => {
                    central.stop_scan().await.ok();
                    let target = self.device_addr.clone()
                        .unwrap_or_else(|| self.device_name.clone());
                    error!("connect: scan timed out after {}s — target={target:?}", self.scan_timeout_secs);
                    return Err(AppError::DeviceNotFound(target));
                }
                Some(event) = events.next() => {
                    let id = match &event {
                        CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => id.clone(),
                        _ => continue,
                    };
                    if let Ok(p) = central.peripheral(&id).await {
                        let props = p.properties().await.ok().flatten().unwrap_or_default();
                        let addr = p.address().to_string();
                        let name = props.local_name.clone().unwrap_or_default();
                        let adv_svcs: Vec<_> = props.services.iter().map(|u| u.to_string()).collect();
                        debug!("connect: event — name={name:?} addr={addr} advertised_services={adv_svcs:?}");

                        if matches_saved_device(
                            &self.device_name,
                            self.device_addr.as_deref(),
                            &name,
                            &addr,
                        ) {
                            debug!("connect: device matched — stopping scan");
                            central.stop_scan().await.ok();
                            break p;
                        }
                    }
                }
            }
        };

        // Physical BLE connection
        debug!("connect: calling peripheral.connect()…");
        peripheral.connect().await?;
        debug!("connect: peripheral.connect() returned Ok");

        // GATT setup — runs after physical connection.  On any fatal error we
        // disconnect the peripheral before propagating so BlueZ doesn't keep a
        // zombie connection that degrades subsequent scans.
        let gatt_result = Self::setup_gatt(&peripheral).await;
        let (write_char, prop_write_char) = match gatt_result {
            Ok(pair) => pair,
            Err(e) => {
                error!("connect: GATT setup failed ({e}) — disconnecting peripheral to clean up");
                peripheral.disconnect().await.ok();
                return Err(e);
            }
        };

        self.write_char = Some(write_char);
        self.prop_write_char = prop_write_char;
        self.peripheral = Some(peripheral);
        debug!("connect: complete — driver ready");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        debug!("disconnect: called");
        if let Some(p) = &self.peripheral {
            p.disconnect().await?;
            debug!("disconnect: peripheral disconnected");
        }
        self.peripheral = None;
        self.write_char = None;
        Ok(())
    }

    async fn write_command(&self, pkt: &[u8]) -> Result<()> {
        debug!("write_command: {:02X?}", pkt);
        let peripheral = self.peripheral.as_ref().ok_or(AppError::NotConnected)?;
        let wc = self.write_char.as_ref().ok_or(AppError::NotConnected)?;
        peripheral.write(wc, pkt, WriteType::WithResponse).await?;
        Ok(())
    }

    async fn ask_stats(&self) -> Result<StatusPacket> {
        self.write_command(&protocol::cmd_ask_stats()).await?;
        // Status arrives via the notification stream (get_notifications).
        Err(AppError::Protocol(
            "ask_stats: response comes via notification stream".into(),
        ))
    }

    fn is_connected(&self) -> bool {
        self.peripheral.is_some()
    }
}

pub fn matches_saved_device(
    saved_name: &str,
    saved_addr: Option<&str>,
    discovered_name: &str,
    discovered_addr: &str,
) -> bool {
    let addr_match = saved_addr.is_some_and(|addr| addr.eq_ignore_ascii_case(discovered_addr));
    let name_match = !discovered_name.is_empty()
        && discovered_name != "Unknown"
        && discovered_name == saved_name;
    addr_match || name_match
}

#[cfg(test)]
mod tests {
    use super::matches_saved_device;

    #[test]
    fn test_matches_saved_device_by_exact_name() {
        assert!(matches_saved_device(
            "WalkingPad",
            None,
            "WalkingPad",
            "AA:BB:CC",
        ));
    }

    #[test]
    fn test_matches_saved_device_rejects_substring_name_match() {
        assert!(!matches_saved_device(
            "WalkingPad",
            None,
            "WalkingPad Pro",
            "AA:BB:CC",
        ));
    }

    #[test]
    fn test_matches_saved_device_by_addr_case_insensitive() {
        assert!(matches_saved_device(
            "Ignored",
            Some("aa:bb:cc"),
            "Unknown",
            "AA:BB:CC",
        ));
    }
}
