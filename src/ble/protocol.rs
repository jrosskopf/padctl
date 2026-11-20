//! WalkingPad BLE protocol: packet building and response parsing.
//!
//! Packet format (commands):
//!   [0xF7, 0xA2, CMD, PARAM, 0xFF, CHECKSUM, 0xFD]
//!
//! Checksum = sum(bytes[1..=4]) % 256
//!
//! Status response prefix: [0xF8, 0xA2] (20 bytes total)

#![allow(dead_code)]

pub const SUFFIX: u8 = 0xFD;
pub const PREFIX_1: u8 = 0xF7;
pub const PREFIX_2: u8 = 0xA2;

pub const CMD_SPEED: u8 = 0x01;
pub const CMD_MODE: u8 = 0x02;
pub const CMD_START: u8 = 0x04;
pub const CMD_STATS: u8 = 0x00;

pub const MODE_MANUAL: u8 = 0x01;
pub const MODE_STANDBY: u8 = 0x02;
pub const MODE_AUTO: u8 = 0x00;

pub const BELT_RUNNING: u8 = 0x01;
pub const BELT_STOPPED: u8 = 0x05;

/// Compute the packet checksum: sum of bytes[1..=4] mod 256.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes[1..bytes.len() - 2]
        .iter()
        .fold(0u16, |acc, &b| acc + b as u16) as u8
}

/// Build a 7-byte command packet.
///
/// Layout: [PREFIX_1, PREFIX_2, cmd, param, extra, checksum, SUFFIX]
pub fn build_command(cmd: u8, param: u8, extra: u8) -> [u8; 7] {
    let mut pkt = [PREFIX_1, PREFIX_2, cmd, param, extra, 0x00, SUFFIX];
    pkt[5] = checksum(&pkt);
    pkt
}

/// Convenience helpers.
pub fn cmd_ask_stats() -> [u8; 7] {
    build_command(CMD_STATS, 0x00, 0xA2)
}

pub fn cmd_set_speed(speed_raw: u8) -> [u8; 7] {
    // speed_raw = km/h * 10, e.g. 30 = 3.0 km/h
    build_command(CMD_SPEED, speed_raw, 0xFF)
}

pub fn cmd_start_belt() -> [u8; 7] {
    build_command(CMD_START, 0x01, 0xFF)
}

pub fn cmd_stop_belt() -> [u8; 7] {
    build_command(CMD_SPEED, 0x00, 0xFF)
}

pub fn cmd_set_mode(mode: u8) -> [u8; 7] {
    build_command(CMD_MODE, mode, 0xFF)
}

/// Decoded status from a treadmill notification packet.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusPacket {
    pub belt_state: u8, // 0x01=running, 0x05=stopped
    pub speed_raw: u8,  // raw; divide by 10 for km/h
    pub time_secs: u32, // cumulative session seconds on device
    pub dist_raw: u32,  // raw; divide by 100 for km
    pub steps: u32,
    /// Device-reported total kcal; `None` when not present in the packet.
    pub kcal: Option<u32>,
}

/// Events pushed by the device on the FTMS Fitness Machine Status characteristic (0x2ADA).
#[derive(Debug, Clone, PartialEq)]
pub enum MachineStatus {
    Reset,
    StoppedBySafetyKey,
    StoppedByUser,
    StartedOrResumed,
    /// New target speed in 0.01 km/h units (set from the device panel).
    SpeedChanged(u16),
    ControlPermissionLost,
    Unknown(u8),
}

/// Parse an FTMS Fitness Machine Status notification (characteristic 0x2ADA).
pub fn parse_machine_status(bytes: &[u8]) -> MachineStatus {
    match bytes.first() {
        Some(0x01) => MachineStatus::Reset,
        // FTMS spec: 0x02 = Stopped or Paused by the User (param 0x01 stop / 0x02 pause),
        //            0x03 = Stopped by Safety Key.
        Some(0x02) => MachineStatus::StoppedByUser,
        Some(0x03) => MachineStatus::StoppedBySafetyKey,
        Some(0x04) => MachineStatus::StartedOrResumed,
        Some(0x05) => {
            let speed = if bytes.len() >= 3 {
                u16::from_le_bytes([bytes[1], bytes[2]])
            } else {
                0
            };
            MachineStatus::SpeedChanged(speed)
        }
        Some(0xFF) => MachineStatus::ControlPermissionLost,
        Some(&b) => MachineStatus::Unknown(b),
        None => MachineStatus::Unknown(0),
    }
}

impl StatusPacket {
    pub fn speed_kmh(&self) -> f32 {
        self.speed_raw as f32 / 10.0
    }

    pub fn distance_km(&self) -> f32 {
        self.dist_raw as f32 / 100.0
    }

    pub fn is_running(&self) -> bool {
        self.belt_state == BELT_RUNNING
    }
}

/// Parse an FTMS Treadmill Data notification (characteristic 0x2ACD).
///
/// Units are normalised to match `StatusPacket` conventions so the rest of
/// the app (stats accumulation, TUI display) needs no changes:
///   - `speed_raw`  : 0.1 km/h  (FTMS gives 0.01 km/h → divide by 10)
///   - `dist_raw`   : 0.01 km   (FTMS gives meters → divide by 10)
///   - `time_secs`  : seconds
///   - `steps`      : 0 (FTMS treadmill data has no step count field)
///   - `belt_state` : inferred from speed (>0 ⟹ BELT_RUNNING)
pub fn parse_ftms_treadmill_data(bytes: &[u8]) -> crate::error::Result<StatusPacket> {
    if bytes.len() < 2 {
        return Err(crate::error::AppError::Protocol(
            "FTMS treadmill data too short".into(),
        ));
    }
    let flags = u16::from_le_bytes([bytes[0], bytes[1]]);
    let mut off = 2usize;

    // Bit 0 = 0  → Instantaneous Speed (uint16 LE, 0.01 km/h) is present
    // Bit 0 = 1  → "More Data" flag; speed absent
    let speed_hundredths: u16 = if flags & 0x0001 == 0 {
        if off + 2 > bytes.len() {
            return Err(crate::error::AppError::Protocol(
                "FTMS: truncated at speed".into(),
            ));
        }
        let v = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
        off += 2;
        v
    } else {
        0
    };
    // Convert 0.01 km/h → 0.1 km/h
    let speed_raw = (speed_hundredths / 10) as u8;

    // Bit 1 → Average Speed (uint16) — skip
    if flags & 0x0002 != 0 {
        off += 2;
    }

    // Bit 2 → Total Distance (uint24 LE, meters) — present
    let dist_meters: u32 = if flags & 0x0004 != 0 {
        if off + 3 > bytes.len() {
            return Err(crate::error::AppError::Protocol(
                "FTMS: truncated at distance".into(),
            ));
        }
        let v = u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], 0]);
        off += 3;
        v
    } else {
        0
    };
    // Convert meters → 0.01 km units (10 m per unit)
    let dist_raw = dist_meters / 10;

    // Bit 3 → Inclination + Ramp Angle (4 bytes) — skip
    if flags & 0x0008 != 0 {
        off += 4;
    }
    // Bit 4 → Elevation Gain (4 bytes) — skip
    if flags & 0x0010 != 0 {
        off += 4;
    }
    // Bit 5 → Instantaneous Pace (uint16, 2 bytes) — skip
    if flags & 0x0020 != 0 {
        off += 2;
    }
    // Bit 6 → Average Pace (uint16, 2 bytes) — skip
    if flags & 0x0040 != 0 {
        off += 2;
    }
    // Bit 7 → Expended Energy: Total kcal (uint16) + Per-Hour (uint16) + Per-Min (uint8) = 5 bytes
    let kcal: Option<u32> = if flags & 0x0080 != 0 {
        if off + 5 > bytes.len() {
            return Err(crate::error::AppError::Protocol(
                "FTMS: truncated at expended energy".into(),
            ));
        }
        let total_kcal = u16::from_le_bytes([bytes[off], bytes[off + 1]]) as u32;
        off += 5;
        Some(total_kcal)
    } else {
        None
    };
    // Bit 8 → Heart Rate (uint8, 1 byte) — skip
    if flags & 0x0100 != 0 {
        off += 1;
    }
    // Bit 9 → Metabolic Equivalent (uint8, 1 byte) — skip
    if flags & 0x0200 != 0 {
        off += 1;
    }

    // Bit 10 → Elapsed Time (uint16 LE, seconds)
    let time_secs: u32 = if flags & 0x0400 != 0 {
        if off + 2 > bytes.len() {
            return Err(crate::error::AppError::Protocol(
                "FTMS: truncated at elapsed time".into(),
            ));
        }
        let v = u16::from_le_bytes([bytes[off], bytes[off + 1]]) as u32;
        off += 2;
        v
    } else {
        0
    };

    // Bit 11 → Remaining Time (uint16, 2 bytes) — skip
    if flags & 0x0800 != 0 {
        off += 2;
    }
    // Bit 12 → Force on Belt + Power Output (sint16 + uint16 = 4 bytes) — skip
    if flags & 0x1000 != 0 {
        off += 4;
    }
    // Bit 13 → KingSmith proprietary extension: cumulative step count (uint24 LE, 3 bytes).
    // Not in the FTMS standard; this device sets bit 13 and appends steps after the standard fields.
    let steps: u32 = if flags & 0x2000 != 0 && off + 3 <= bytes.len() {
        u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], 0])
    } else {
        0
    };

    let belt_state = if speed_raw > 0 {
        BELT_RUNNING
    } else {
        BELT_STOPPED
    };

    Ok(StatusPacket {
        belt_state,
        speed_raw,
        time_secs,
        dist_raw,
        steps,
        kcal,
    })
}

/// Parse a 20-byte status notification (legacy proprietary protocol).
///
/// Expected prefix: [0xF8, 0xA2]
pub fn parse_status(bytes: &[u8]) -> crate::error::Result<StatusPacket> {
    if bytes.len() < 18 {
        return Err(crate::error::AppError::Protocol(format!(
            "status packet too short: {} bytes",
            bytes.len()
        )));
    }
    if bytes[0] != 0xF8 || bytes[1] != 0xA2 {
        return Err(crate::error::AppError::Protocol(format!(
            "unexpected prefix: [{:#04x}, {:#04x}]",
            bytes[0], bytes[1]
        )));
    }

    let belt_state = bytes[2];
    let speed_raw = bytes[3];
    let time_secs = decode3(bytes[5], bytes[6], bytes[7]);
    let dist_raw = decode3(bytes[8], bytes[9], bytes[10]);
    let steps = decode3(bytes[11], bytes[12], bytes[13]);

    Ok(StatusPacket {
        belt_state,
        speed_raw,
        time_secs,
        dist_raw,
        steps,
        kcal: None,
    })
}

#[inline]
fn decode3(b0: u8, b1: u8, b2: u8) -> u32 {
    (b0 as u32) << 16 | (b1 as u32) << 8 | b2 as u32
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Iter 1: packet building ───────────────────────────────────────────

    #[test]
    fn test_build_speed_cmd_30_kmh() {
        // 3.0 km/h → speed_raw = 30
        // pkt = [F7, A2, 01, 1E, FF, CK, FD]
        // CK = (A2 + 01 + 1E + FF) % 256 = (162+1+30+255) % 256 = 448 % 256 = 192
        let pkt = cmd_set_speed(30);
        assert_eq!(pkt, [0xF7, 0xA2, 0x01, 0x1E, 0xFF, 192, 0xFD]);
    }

    #[test]
    fn test_build_speed_cmd_60_max() {
        // 6.0 km/h → speed_raw = 60
        // CK = (162 + 1 + 60 + 255) % 256 = 478 % 256 = 222
        let pkt = cmd_set_speed(60);
        assert_eq!(pkt, [0xF7, 0xA2, 0x01, 0x3C, 0xFF, 222, 0xFD]);
    }

    #[test]
    fn test_build_stop_belt() {
        // stop = set speed 0
        // CK = (162 + 1 + 0 + 255) % 256 = 418 % 256 = 162
        let pkt = cmd_stop_belt();
        assert_eq!(pkt, [0xF7, 0xA2, 0x01, 0x00, 0xFF, 162, 0xFD]);
    }

    #[test]
    fn test_build_start_belt() {
        // [F7, A2, 04, 01, FF, CK, FD]
        // CK = (162 + 4 + 1 + 255) % 256 = 422 % 256 = 166
        let pkt = cmd_start_belt();
        assert_eq!(pkt, [0xF7, 0xA2, 0x04, 0x01, 0xFF, 166, 0xFD]);
    }

    #[test]
    fn test_build_manual_mode() {
        // [F7, A2, 02, 01, FF, CK, FD]
        // CK = (162 + 2 + 1 + 255) % 256 = 420 % 256 = 164
        let pkt = cmd_set_mode(MODE_MANUAL);
        assert_eq!(pkt, [0xF7, 0xA2, 0x02, 0x01, 0xFF, 164, 0xFD]);
    }

    #[test]
    fn test_build_standby_mode() {
        // CK = (162 + 2 + 2 + 255) % 256 = 421 % 256 = 165
        let pkt = cmd_set_mode(MODE_STANDBY);
        assert_eq!(pkt, [0xF7, 0xA2, 0x02, 0x02, 0xFF, 165, 0xFD]);
    }

    #[test]
    fn test_build_ask_stats() {
        // [F7, A2, 00, 00, A2, CK, FD]
        // CK = (162 + 0 + 0 + 162) % 256 = 324 % 256 = 68
        let pkt = cmd_ask_stats();
        assert_eq!(pkt, [0xF7, 0xA2, 0x00, 0x00, 0xA2, 68, 0xFD]);
    }

    // ── Iter 2: checksum ─────────────────────────────────────────────────

    #[test]
    fn test_checksum_overflow_wraps() {
        // bytes[1..=4] that sum > 255
        let pkt = [0xF7, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0xFD];
        // sum = 255*4 = 1020; 1020 % 256 = 252
        assert_eq!(checksum(&pkt), 252);
    }

    #[test]
    fn test_checksum_zero_params() {
        let pkt = [0xF7, 0xA2, 0x00, 0x00, 0x00, 0x00, 0xFD];
        assert_eq!(checksum(&pkt), 162);
    }

    // ── FTMS treadmill data parsing ───────────────────────────────────────

    /// Build a minimal FTMS Treadmill Data packet with speed + distance + time.
    /// flags: bit0=0 (speed present), bit2=1 (distance), bit10=1 (elapsed time)
    /// → flags = 0x0404
    fn make_ftms_fixture(speed_hundredths: u16, dist_meters: u32, time_secs: u16) -> Vec<u8> {
        let flags: u16 = 0x0404; // speed + distance + elapsed time
        let mut b = Vec::new();
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&speed_hundredths.to_le_bytes());
        b.extend_from_slice(&[
            (dist_meters & 0xFF) as u8,
            ((dist_meters >> 8) & 0xFF) as u8,
            ((dist_meters >> 16) & 0xFF) as u8,
        ]);
        b.extend_from_slice(&time_secs.to_le_bytes());
        b
    }

    #[test]
    fn test_ftms_parse_running_3kmh() {
        // 3.0 km/h = 300 in 0.01 km/h units
        // 1000 m distance, 95 s elapsed
        let bytes = make_ftms_fixture(300, 1000, 95);
        let st = parse_ftms_treadmill_data(&bytes).unwrap();
        // speed_raw = 300/10 = 30 → 3.0 km/h
        assert_eq!(st.speed_raw, 30);
        assert!((st.speed_kmh() - 3.0).abs() < f32::EPSILON);
        // dist_raw = 1000/10 = 100 → 1.00 km
        assert_eq!(st.dist_raw, 100);
        assert!((st.distance_km() - 1.0).abs() < 0.001);
        assert_eq!(st.time_secs, 95);
        assert_eq!(st.kcal, None); // energy bit not set in fixture
        assert_eq!(st.belt_state, BELT_RUNNING);
        assert!(st.is_running());
    }

    #[test]
    fn test_ftms_parse_stopped() {
        let bytes = make_ftms_fixture(0, 0, 0);
        let st = parse_ftms_treadmill_data(&bytes).unwrap();
        assert_eq!(st.speed_raw, 0);
        assert_eq!(st.belt_state, BELT_STOPPED);
        assert!(!st.is_running());
    }

    #[test]
    fn test_ftms_parse_speed_only() {
        // flags = 0x0000: only speed present
        let flags: u16 = 0x0000;
        let mut bytes = flags.to_le_bytes().to_vec();
        bytes.extend_from_slice(&150u16.to_le_bytes()); // 1.5 km/h
        let st = parse_ftms_treadmill_data(&bytes).unwrap();
        assert_eq!(st.speed_raw, 15); // 150/10 = 15 → 1.5 km/h
        assert_eq!(st.dist_raw, 0);
        assert_eq!(st.time_secs, 0);
        assert_eq!(st.kcal, None);
    }

    #[test]
    fn test_ftms_parse_too_short() {
        assert!(parse_ftms_treadmill_data(&[0x00]).is_err());
    }

    #[test]
    fn test_ftms_parse_skips_optional_fields() {
        // bit0=0: speed present, bit1: avg speed (2 B), bit2: distance (3 B),
        // bit3: inclination+ramp (4 B), bit7: expended energy (5 B), bit10: elapsed time (2 B)
        // flags = 0x0002 | 0x0004 | 0x0008 | 0x0080 | 0x0400 = 0x048E
        let flags: u16 = 0x048E;
        let mut b = Vec::new();
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&200u16.to_le_bytes()); // speed = 200 (2.0 km/h)
        b.extend_from_slice(&100u16.to_le_bytes()); // avg speed (skipped, 2 B)
        b.extend_from_slice(&[0xE8, 0x03, 0x00]); // distance = 1000 m (3 B)
        b.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // inclination + ramp (skipped, 4 B)
        b.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00]); // expended energy (skipped, 5 B)
        b.extend_from_slice(&60u16.to_le_bytes()); // elapsed = 60 s (2 B)
        let st = parse_ftms_treadmill_data(&b).unwrap();
        assert_eq!(st.speed_raw, 20); // 200/10 = 20 → 2.0 km/h
        assert_eq!(st.dist_raw, 100); // 1000/10 = 100
        assert_eq!(st.time_secs, 60);
        assert_eq!(st.kcal, Some(0)); // energy bytes all zero in fixture
    }

    #[test]
    fn test_ftms_parse_kingsmith_bit13_steps() {
        // Real packet captured from KS-HD-Z1D (first notification in session log).
        // flags=0x2484: speed(bit0=0), distance(bit2), energy(bit7), elapsed(bit10), steps(bit13)
        let raw = [
            0x84u8, 0x24, // flags = 0x2484
            0xF4, 0x01, // speed = 500 (5.0 km/h)
            0xD2, 0x05, 0x00, // distance = 1490 m → dist_raw = 149
            0x68, 0x00, 0x00, 0x00, 0x00, // energy: total=104 kcal, per-hr=0, per-min=0
            0x3A, 0x05, // elapsed = 0x053A = 1338 s
            0x06, 0x09, 0x00, // steps = 0x000906 = 2310
        ];
        let st = parse_ftms_treadmill_data(&raw).unwrap();
        assert_eq!(st.speed_raw, 50); // 500/10
        assert_eq!(st.dist_raw, 149); // 1490/10
        assert_eq!(st.time_secs, 1338);
        assert_eq!(st.kcal, Some(104));
        assert_eq!(st.steps, 2310);
        assert!(st.is_running());
    }

    #[test]
    fn test_ftms_parse_with_device_kcal() {
        // flags: speed (bit0=0) + distance (bit2) + energy (bit7) + elapsed time (bit10)
        // = 0x0004 | 0x0080 | 0x0400 = 0x0484
        let flags: u16 = 0x0484;
        let mut b = Vec::new();
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&300u16.to_le_bytes()); // speed = 3.0 km/h
        b.extend_from_slice(&[0x64, 0x00, 0x00]); // distance = 100 m
                                                  // Expended Energy: total=42 kcal, per-hour=180, per-min=3
        b.extend_from_slice(&42u16.to_le_bytes()); // total kcal
        b.extend_from_slice(&180u16.to_le_bytes()); // per hour (ignored)
        b.push(3); // per minute (ignored)
        b.extend_from_slice(&120u16.to_le_bytes()); // elapsed = 120 s
        let st = parse_ftms_treadmill_data(&b).unwrap();
        assert_eq!(st.kcal, Some(42));
        assert_eq!(st.time_secs, 120);
    }

    // ── FTMS Fitness Machine Status (0x2ADA) ───────────────────────────────

    #[test]
    fn test_machine_status_codes_follow_ftms_spec() {
        // Pause from the remote / app (captured on a KingSmith R3, KS-AP-RQ3)
        assert_eq!(
            parse_machine_status(&[0x02, 0x02]),
            MachineStatus::StoppedByUser
        );
        // Session ended
        assert_eq!(
            parse_machine_status(&[0x02, 0x01]),
            MachineStatus::StoppedByUser
        );
        assert_eq!(
            parse_machine_status(&[0x03]),
            MachineStatus::StoppedBySafetyKey
        );
        assert_eq!(
            parse_machine_status(&[0x04]),
            MachineStatus::StartedOrResumed
        );
        assert_eq!(
            parse_machine_status(&[0x05, 0x58, 0x02]),
            MachineStatus::SpeedChanged(600)
        );
    }

    // ── Iter 3: status parsing ───────────────────────────────────────────

    fn make_status_fixture(belt: u8, speed: u8, secs: u32, dist: u32, steps: u32) -> Vec<u8> {
        let mut b = vec![0u8; 20];
        b[0] = 0xF8;
        b[1] = 0xA2;
        b[2] = belt;
        b[3] = speed;
        b[4] = 0x01; // manual mode
        b[5] = (secs >> 16) as u8;
        b[6] = (secs >> 8) as u8;
        b[7] = secs as u8;
        b[8] = (dist >> 16) as u8;
        b[9] = (dist >> 8) as u8;
        b[10] = dist as u8;
        b[11] = (steps >> 16) as u8;
        b[12] = (steps >> 8) as u8;
        b[13] = steps as u8;
        b
    }

    #[test]
    fn test_parse_status_running_3kmh() {
        let bytes = make_status_fixture(BELT_RUNNING, 30, 95, 9, 184);
        let st = parse_status(&bytes).unwrap();
        assert_eq!(st.belt_state, BELT_RUNNING);
        assert_eq!(st.speed_raw, 30);
        assert!((st.speed_kmh() - 3.0).abs() < f32::EPSILON);
        assert_eq!(st.time_secs, 95);
        assert_eq!(st.dist_raw, 9);
        assert!((st.distance_km() - 0.09).abs() < 0.001);
        assert_eq!(st.steps, 184);
        assert!(st.is_running());
    }

    #[test]
    fn test_parse_status_stopped() {
        let bytes = make_status_fixture(BELT_STOPPED, 0, 99, 9, 194);
        let st = parse_status(&bytes).unwrap();
        assert!(!st.is_running());
        assert_eq!(st.speed_raw, 0);
    }

    #[test]
    fn test_parse_status_too_short_returns_err() {
        let bytes = vec![0xF8, 0xA2, 0x01];
        assert!(parse_status(&bytes).is_err());
    }

    #[test]
    fn test_parse_status_wrong_prefix_returns_err() {
        let bytes = make_status_fixture(1, 30, 0, 0, 0);
        let mut bad = bytes.clone();
        bad[0] = 0x00;
        assert!(parse_status(&bad).is_err());
    }

    #[test]
    fn test_parse_status_large_values() {
        // 3-byte max = 16,777,215
        let bytes = make_status_fixture(BELT_RUNNING, 60, 7200, 1000, 10000);
        let st = parse_status(&bytes).unwrap();
        assert_eq!(st.time_secs, 7200); // 2 hours
        assert_eq!(st.dist_raw, 1000); // 10.00 km
        assert_eq!(st.steps, 10000);
    }
}
