use std::time::{Duration, Instant, SystemTime};

use crate::ble::protocol::{BELT_RUNNING, BELT_STOPPED};
use crate::session::{SessionState, SessionEvent};
use crate::session::stats::Stats;
use crate::types::DiscoveredDevice;

/// Number of render ticks (×100 ms) the pump pulse lasts.
const PUMP_FRAMES: u8 = 4;

/// Maximum gap between two keypresses to count as a double-tap.
pub const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(300);

/// High-level application mode — drives which render path and keybindings are active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Scanning for nearby BLE devices; user picks one.
    Scanning,
    /// Connecting to a known device address.
    Connecting,
    /// Session UI: belt control + live stats.
    Session,
    /// Session has ended; showing summary + optional Google Fit sync prompt.
    SessionSummary,
}

/// Number of focusable items in the config dialog.
pub const CONFIG_DIALOG_ITEMS: usize = 4;

/// Config dialog state — tracks focus and the text input buffers.
#[derive(Debug, Default)]
pub struct ConfigDialog {
    pub visible: bool,
    /// Focused item: 0=Unpair  1=ClientId  2=ClientSecret  3=Save
    pub selected: usize,
    pub input_client_id: String,
    pub input_client_secret: String,
    /// Human-readable description of the currently saved device.
    pub device_display: String,
    /// Whether a Fit refresh token is already persisted.
    pub fit_token_saved: bool,
}

/// Speed-input popup state — collects up to two digits (raw 0.1 km/h units)
/// for a direct "go to speed" entry while the belt is running.
#[derive(Debug, Default)]
pub struct SpeedInput {
    pub visible: bool,
    /// Up to 2 chars, raw tenths of km/h (e.g. "35" → 3.5 km/h).
    pub digits: String,
}

impl SpeedInput {
    /// Parsed raw speed (0.1 km/h units), or None if the buffer is empty/invalid.
    pub fn parse_raw(&self) -> Option<u8> {
        self.digits.parse::<u8>().ok()
    }
}

/// Snapshot of session stats captured when the session ends.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub duration_secs: u32,
    pub steps: u32,
    pub distance_km: f32,
    pub calories: f32,
    /// Wall-clock time when the session started (for Fit timestamps).
    pub started_at: SystemTime,
    /// Wall-clock time when the session ended.
    pub ended_at: SystemTime,
}

/// Shared application state, updated by BLE notifications and read by the renderer.
#[derive(Debug)]
pub struct AppState {
    pub mode: AppMode,
    pub session: SessionState,
    pub stats: Stats,
    pub ble_connected: bool,
    pub device_name: String,
    /// Address of the device we are connecting / connected to.
    pub connecting_addr: Option<String>,
    pub resume_speed_raw: u8,
    pub max_speed_raw: u8,
    pub min_speed_raw: u8,
    pub speed_step_raw: u8,
    pub log_lines: Vec<String>,
    /// Devices discovered during the current scan.
    pub scan_results: Vec<DiscoveredDevice>,
    /// Index into `scan_results` for keyboard navigation.
    pub scan_cursor: usize,
    /// Belt state from the most-recent FTMS notification; `None` before first packet.
    pub last_belt_state: Option<u8>,

    // ── Session summary ───────────────────────────────────────────────────
    /// Wall-clock time when the current session started; set when SessionEvent::Start fires.
    pub session_start_at: Option<SystemTime>,
    /// Snapshot captured when the session ends; displayed in SessionSummary mode.
    pub summary: Option<SessionSummary>,
    /// Whether Google Fit sync is available (client_id + client_secret configured).
    pub fit_client_id: Option<String>,
    pub fit_client_secret: Option<String>,

    // ── Config dialog ─────────────────────────────────────────────────────
    pub config_dialog: ConfigDialog,

    // ── Speed-input popup ─────────────────────────────────────────────────
    pub speed_input: SpeedInput,

    // ── Double-tap tracking ───────────────────────────────────────────────
    /// When the last speed-up (↑/k) keypress fired; used for double-tap detection.
    pub last_up_at: Option<Instant>,
    /// When the last speed-down (↓/j) keypress fired; used for double-tap detection.
    pub last_down_at: Option<Instant>,

    // ── Animation state ───────────────────────────────────────────────────
    /// Disc rotation angle in radians, advanced each tick proportional to speed.
    pub disc_angle: f64,
    /// Countdown frames for the "pump" pulse effect when a step is detected.
    /// Set to PUMP_FRAMES on each new step, counts down to 0 each render tick.
    pub pump_frames: u8,
    /// Step count from the previous tick — used to detect new steps.
    pub prev_steps: u32,
}

impl AppState {
    pub fn new(device_name: String) -> Self {
        Self {
            mode: AppMode::Scanning,
            session: SessionState::Idle,
            stats: Stats::new(),
            ble_connected: false,
            device_name,
            connecting_addr: None,
            resume_speed_raw: 20,
            max_speed_raw: 60,
            min_speed_raw: 10,
            speed_step_raw: 1,
            log_lines: Vec::new(),
            scan_results: Vec::new(),
            scan_cursor: 0,
            last_belt_state: None,
            session_start_at: None,
            summary: None,
            fit_client_id: None,
            fit_client_secret: None,
            config_dialog: ConfigDialog::default(),
            speed_input: SpeedInput::default(),
            last_up_at: None,
            last_down_at: None,
            disc_angle: 0.0,
            pump_frames: 0,
            prev_steps: 0,
        }
    }

    // ── Scan list helpers ──────────────────────────────────────────────────

    /// Add or update a discovered device. Deduplicates by `addr`.
    /// On duplicate: updates RSSI if the new value is Some; updates name if was empty/Unknown.
    pub fn add_scan_result(&mut self, device: DiscoveredDevice) {
        if let Some(existing) = self.scan_results.iter_mut().find(|d| d.addr == device.addr) {
            if device.rssi.is_some() {
                existing.rssi = device.rssi;
            }
            if existing.name.is_empty() || existing.name == "Unknown" {
                existing.name = device.name;
            }
        } else {
            self.scan_results.push(device);
        }
    }

    /// Clear scan results and reset cursor (e.g. on rescan).
    pub fn clear_scan_results(&mut self) {
        self.scan_results.clear();
        self.scan_cursor = 0;
    }

    /// Move the scan cursor up (towards index 0), clamped.
    pub fn scan_cursor_up(&mut self) {
        if self.scan_results.is_empty() { return; }
        self.scan_cursor = self.scan_cursor.saturating_sub(1);
    }

    /// Move the scan cursor down (towards last index), clamped.
    pub fn scan_cursor_down(&mut self) {
        if self.scan_results.is_empty() { return; }
        let max = self.scan_results.len().saturating_sub(1);
        if self.scan_cursor < max {
            self.scan_cursor += 1;
        }
    }

    /// Return the currently highlighted device, if any.
    pub fn selected_scan_device(&self) -> Option<&DiscoveredDevice> {
        self.scan_results.get(self.scan_cursor)
    }

    // ── Session helpers ────────────────────────────────────────────────────

    pub fn apply_session_event(&mut self, event: SessionEvent) {
        match self.session.transition(event) {
            Ok(next) => self.session = next,
            Err(e) => self.push_log(format!("ERR: {e}")),
        }
    }

    /// Advance animation state by one render tick (100 ms).
    ///
    /// - Rotates the disc by an angle proportional to current speed.
    /// - Detects new steps and triggers the pump pulse.
    pub fn tick_animation(&mut self) {
        use std::f64::consts::TAU;
        // At max speed (6.0 km/h, raw=60) we want ~1 full rotation per 2 seconds
        // = TAU / 20 ticks.  At lower speeds, proportionally slower.
        let speed_frac = (self.stats.current_speed_kmh / 6.0) as f64;
        self.disc_angle = (self.disc_angle + speed_frac * TAU / 20.0) % TAU;

        // Detect step increase → trigger pump
        let cur = self.stats.total_steps;
        if cur > self.prev_steps {
            self.pump_frames = PUMP_FRAMES;
        }
        self.prev_steps = cur;

        if self.pump_frames > 0 {
            self.pump_frames -= 1;
        }
    }

    pub fn open_config_dialog(
        &mut self,
        device_display: String,
        fit_client_id: String,
        fit_client_secret: String,
        fit_token_saved: bool,
    ) {
        self.config_dialog = ConfigDialog {
            visible: true,
            selected: 1,
            input_client_id: fit_client_id,
            input_client_secret: fit_client_secret,
            device_display,
            fit_token_saved,
        };
    }

    pub fn fit_configured(&self) -> bool {
        self.fit_client_id.is_some() && self.fit_client_secret.is_some()
    }

    pub fn sync_session_to_belt(&mut self, belt_state: u8, now: SystemTime) {
        match (self.session, belt_state) {
            (SessionState::Idle, BELT_RUNNING) => {
                self.session_start_at.get_or_insert(now);
                self.apply_session_event(SessionEvent::Start);
            }
            (SessionState::Paused, BELT_RUNNING) => {
                self.apply_session_event(SessionEvent::Resume);
            }
            (SessionState::Running, BELT_STOPPED) => {
                self.apply_session_event(SessionEvent::Pause);
            }
            _ => {}
        }
    }

    pub fn push_log(&mut self, msg: impl Into<String>) {
        let line = msg.into();
        self.log_lines.push(line);
        if self.log_lines.len() > 20 {
            self.log_lines.remove(0);
        }
    }

    pub fn status_label(&self) -> &'static str {
        match self.session {
            SessionState::Idle    => "IDLE",
            SessionState::Running => "RUNNING",
            SessionState::Paused  => "PAUSED",
        }
    }

    pub fn format_time(secs: u32) -> String {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h}:{m:02}:{s:02}")
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::protocol::{BELT_RUNNING, BELT_STOPPED};

    fn dev(name: &str, addr: &str, rssi: Option<i16>) -> DiscoveredDevice {
        DiscoveredDevice { name: name.into(), addr: addr.into(), rssi }
    }

    // ── Iter 4a: cursor bounds ────────────────────────────────────────────

    #[test]
    fn test_scan_cursor_no_underflow_on_empty() {
        let mut s = AppState::new("WalkingPad".into());
        s.scan_cursor_up();
        assert_eq!(s.scan_cursor, 0);
    }

    #[test]
    fn test_scan_cursor_down_no_overflow_on_empty() {
        let mut s = AppState::new("WalkingPad".into());
        s.scan_cursor_down();
        assert_eq!(s.scan_cursor, 0);
    }

    #[test]
    fn test_scan_cursor_clamps_at_top() {
        let mut s = AppState::new("WalkingPad".into());
        s.scan_results = vec![dev("A", "AA:01", None), dev("B", "AA:02", None)];
        s.scan_cursor = 0;
        s.scan_cursor_up();
        assert_eq!(s.scan_cursor, 0);
    }

    #[test]
    fn test_scan_cursor_clamps_at_bottom() {
        let mut s = AppState::new("WalkingPad".into());
        s.scan_results = vec![dev("A", "AA:01", None), dev("B", "AA:02", None)];
        s.scan_cursor = 1;
        s.scan_cursor_down();
        assert_eq!(s.scan_cursor, 1);
    }

    #[test]
    fn test_scan_cursor_moves_up_and_down() {
        let mut s = AppState::new("WalkingPad".into());
        s.scan_results = vec![dev("A", "AA:01", None), dev("B", "AA:02", None), dev("C", "AA:03", None)];
        s.scan_cursor_down();
        assert_eq!(s.scan_cursor, 1);
        s.scan_cursor_down();
        assert_eq!(s.scan_cursor, 2);
        s.scan_cursor_up();
        assert_eq!(s.scan_cursor, 1);
    }

    // ── Iter 5a: selected device ──────────────────────────────────────────

    #[test]
    fn test_selected_device_on_empty_is_none() {
        let s = AppState::new("WalkingPad".into());
        assert!(s.selected_scan_device().is_none());
    }

    #[test]
    fn test_selected_device_returns_cursor_item() {
        let mut s = AppState::new("WalkingPad".into());
        s.scan_results = vec![dev("A", "AA:01", None), dev("B", "AA:02", None)];
        s.scan_cursor = 1;
        assert_eq!(s.selected_scan_device().unwrap().addr, "AA:02");
    }

    // ── Iter 5b: deduplication ────────────────────────────────────────────

    #[test]
    fn test_add_scan_result_dedup_same_addr() {
        let mut s = AppState::new("WalkingPad".into());
        s.add_scan_result(dev("Pad", "AA:01", Some(-60)));
        s.add_scan_result(dev("Pad", "AA:01", Some(-55))); // RSSI update
        assert_eq!(s.scan_results.len(), 1);
        assert_eq!(s.scan_results[0].rssi, Some(-55));
    }

    #[test]
    fn test_add_scan_result_updates_name_from_unknown() {
        let mut s = AppState::new("WalkingPad".into());
        s.add_scan_result(dev("Unknown", "AA:01", None));
        s.add_scan_result(dev("KingSmith Z1", "AA:01", Some(-60)));
        assert_eq!(s.scan_results[0].name, "KingSmith Z1");
    }

    #[test]
    fn test_add_scan_result_different_addr_appends() {
        let mut s = AppState::new("WalkingPad".into());
        s.add_scan_result(dev("A", "AA:01", None));
        s.add_scan_result(dev("B", "AA:02", None));
        assert_eq!(s.scan_results.len(), 2);
    }

    #[test]
    fn test_clear_scan_results_resets_cursor() {
        let mut s = AppState::new("WalkingPad".into());
        s.scan_results = vec![dev("A", "AA:01", None)];
        s.scan_cursor = 1;
        s.clear_scan_results();
        assert!(s.scan_results.is_empty());
        assert_eq!(s.scan_cursor, 0);
    }

    #[test]
    fn test_sync_belt_running_starts_idle_session() {
        let mut s = AppState::new("WalkingPad".into());
        s.sync_session_to_belt(BELT_RUNNING, SystemTime::UNIX_EPOCH);
        assert_eq!(s.session, SessionState::Running);
        assert_eq!(s.session_start_at, Some(SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn test_sync_belt_running_resumes_paused_session_without_resetting_start() {
        let mut s = AppState::new("WalkingPad".into());
        let started_at = SystemTime::UNIX_EPOCH;
        s.session = SessionState::Paused;
        s.session_start_at = Some(started_at);

        s.sync_session_to_belt(BELT_RUNNING, SystemTime::UNIX_EPOCH + Duration::from_secs(10));

        assert_eq!(s.session, SessionState::Running);
        assert_eq!(s.session_start_at, Some(started_at));
    }

    // ── Speed-input popup ─────────────────────────────────────────────────

    #[test]
    fn test_speed_input_parse_raw_two_digits() {
        let si = SpeedInput { visible: true, digits: "35".into() };
        assert_eq!(si.parse_raw(), Some(35));
    }

    #[test]
    fn test_speed_input_parse_raw_empty_is_none() {
        let si = SpeedInput { visible: true, digits: String::new() };
        assert_eq!(si.parse_raw(), None);
    }

    #[test]
    fn test_speed_input_clamp_below_min() {
        let s = AppState::new("WalkingPad".into());
        let raw = 5u8.clamp(s.min_speed_raw, s.max_speed_raw);
        assert_eq!(raw, s.min_speed_raw); // 1.0 km/h
    }

    #[test]
    fn test_speed_input_clamp_above_max() {
        let s = AppState::new("WalkingPad".into());
        let raw = 99u8.clamp(s.min_speed_raw, s.max_speed_raw);
        assert_eq!(raw, s.max_speed_raw); // 6.0 km/h
    }

    #[test]
    fn test_speed_input_clamp_in_range() {
        let s = AppState::new("WalkingPad".into());
        let raw = 35u8.clamp(s.min_speed_raw, s.max_speed_raw);
        assert_eq!(raw, 35);
    }

    #[test]
    fn test_sync_belt_stopped_pauses_running_session() {
        let mut s = AppState::new("WalkingPad".into());
        s.session = SessionState::Running;

        s.sync_session_to_belt(BELT_STOPPED, SystemTime::UNIX_EPOCH);

        assert_eq!(s.session, SessionState::Paused);
    }
}
