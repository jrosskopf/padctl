mod ble;
mod config;
mod error;
mod fit;
mod logger;
mod session;
mod telemetry;
mod types;
mod ui;

use std::io;
use std::time::Duration;

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use log::{debug, error, info, warn};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{mpsc, watch};
use tokio::time;

use ble::protocol;
use ble::scanner::{
    matches_saved_device, scan_devices, BleDriver, FITNESS_MACHINE_STATUS_UUID,
    PROPRIETARY_NOTIFY_UUID, TREADMILL_DATA_UUID,
};
use ble::traits::WalkingPadBle;
use config::AppConfig;
use session::{SessionEvent, SessionState};
use types::{BleEvent, Cmd};
use ui::app::{AppMode, AppState, SessionSummary, CONFIG_DIALOG_ITEMS, DOUBLE_TAP_WINDOW};
use ui::render;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(
    version,
    name = "padctl",
    about = "Cypherpunk TUI for Kingfisher / KingSmith WalkingPad"
)]
struct Cli {
    /// BLE advertised device name (overrides saved config)
    #[arg(long)]
    device_name: Option<String>,

    /// Connect directly to this MAC address, bypassing scan
    #[arg(long)]
    device_addr: Option<String>,

    /// BLE scan / connection timeout in seconds
    #[arg(long, default_value_t = 10)]
    scan_timeout: u64,

    /// Write debug log to padctl-debug.log in the current directory
    #[arg(long, default_value_t = false)]
    debug: bool,

    /// Disable anonymous usage telemetry for this run. See TELEMETRY.md.
    #[arg(long, default_value_t = false)]
    no_telemetry: bool,
}

/// The only `feature` values padctl may emit.
const PADCTL_FEATURES: &[&str] = &[
    "session_started",
    "session_ended",
    "device_connected",
    "google_fit_synced",
];

/// One line to stderr, once per install, pointing at the policy.
fn telemetry_notice() {
    let marker = dirs_cache().join("padctl-telemetry-notice");
    if marker.exists() {
        return;
    }
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, "1");
    eprintln!(
        "padctl sends anonymous usage telemetry (no device names, addresses or \
         tokens). Disable with --no-telemetry, PADCTL_NO_TELEMETRY=1, or \
         DO_NOT_TRACK=1. See \
         https://github.com/jrosskopf/padctl/blob/main/TELEMETRY.md"
    );
}

fn dirs_cache() -> std::path::PathBuf {
    std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
        })
        .join("padctl")
}

#[cfg(target_os = "linux")]
async fn check_bluez() {
    use btleplug::platform::Manager;

    if let Err(e) = Manager::new().await {
        let msg = e.to_string().to_lowercase();
        let is_bluez_missing = msg.contains("org.bluez")
            || msg.contains("service unknown")
            || msg.contains("bluez")
            || msg.contains("dbus");

        if is_bluez_missing {
            eprintln!(
                "Error: BlueZ is not available on this system.\n\
                 padctl requires the BlueZ Bluetooth stack and its D-Bus service.\n\n\
                 Install it with your package manager:\n\
                 \n  \
                 Ubuntu / Debian:  sudo apt install bluetooth bluez\n  \
                                   sudo systemctl enable --now bluetooth\n  \
                 Arch Linux:       sudo pacman -S bluez bluez-utils\n  \
                                   sudo systemctl enable --now bluetooth\n  \
                 Fedora / RHEL:    sudo dnf install bluez\n  \
                                   sudo systemctl enable --now bluetooth\n\
                 \nOriginal error: {e}"
            );
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let no_telemetry = cli.no_telemetry;

    if cli.debug {
        logger::init()?;
        info!("padctl starting — debug log active");
    }

    #[cfg(target_os = "linux")]
    check_bluez().await;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let telemetry = telemetry::Telemetry::new("padctl", env!("CARGO_PKG_VERSION"), no_telemetry)
        .with_features(PADCTL_FEATURES);
    if telemetry.is_enabled() {
        telemetry_notice();
        telemetry.capture("cli_started", vec![]);
    }

    let result = run(&mut terminal, cli).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // reqwest::blocking panics if called from inside a tokio runtime, so the
    // flush happens on a plain thread. It is best-effort either way: telemetry
    // must never delay or fail the program it is measuring.
    let _ = std::thread::spawn(move || telemetry.flush()).join();

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }
    Ok(())
}

async fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, cli: Cli) -> io::Result<()> {
    let (evt_tx, mut evt_rx) = mpsc::channel::<BleEvent>(64);
    let (fit_log_tx, mut fit_log_rx) = mpsc::channel::<String>(16);
    let mut rt = RuntimeController::new(cli, evt_tx.clone(), fit_log_tx);
    rt.start_initial_scan();

    let tick = Duration::from_millis(100);

    loop {
        while let Ok(evt) = evt_rx.try_recv() {
            rt.handle_ble_event(evt).await;
        }

        while let Ok(msg) = fit_log_rx.try_recv() {
            rt.state.push_log(msg);
        }

        rt.state.tick_animation();
        terminal.draw(|f| render::draw(f, &rt.state))?;

        if !event::poll(tick)? {
            continue;
        }

        if let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read()?
        {
            if rt.should_quit(code, modifiers) {
                rt.request_quit().await;
                break;
            }
            rt.handle_key(code).await;
        }
    }

    Ok(())
}

struct RuntimeController {
    cfg: AppConfig,
    state: AppState,
    evt_tx: mpsc::Sender<BleEvent>,
    fit_log_tx: mpsc::Sender<String>,
    scan_timeout: u64,
    cmd_tx: mpsc::Sender<Cmd>,
    cmd_rx_slot: Option<mpsc::Receiver<Cmd>>,
    scan_stop_tx: Option<watch::Sender<bool>>,
}

impl RuntimeController {
    fn new(cli: Cli, evt_tx: mpsc::Sender<BleEvent>, fit_log_tx: mpsc::Sender<String>) -> Self {
        let (mut cfg, load_warning) = AppConfig::load_with_warning();
        if let Some(name) = cli.device_name {
            cfg.device_name = name;
        }
        if let Some(addr) = cli.device_addr {
            cfg.device_addr = Some(addr);
        }

        let mut state = AppState::new(cfg.device_name.clone());
        state.fit_client_id = cfg.fit_client_id.clone();
        state.fit_client_secret = cfg.fit_client_secret.clone();
        if let Some(msg) = load_warning {
            state.push_log(format!("!! {msg}"));
        }

        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(32);
        let (scan_stop_tx, _) = watch::channel(false);

        Self {
            cfg,
            state,
            evt_tx,
            fit_log_tx,
            scan_timeout: cli.scan_timeout,
            cmd_tx,
            cmd_rx_slot: Some(cmd_rx),
            scan_stop_tx: Some(scan_stop_tx),
        }
    }

    fn should_quit(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
            || (!self.state.config_dialog.visible
                && code == KeyCode::Char('q')
                && modifiers.is_empty()
                && self.state.mode != AppMode::SessionSummary)
    }

    async fn request_quit(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Quit).await;
        self.stop_scan();
    }

    fn start_initial_scan(&mut self) {
        debug!(
            "run: starting scan (saved device_name={:?} device_addr={:?})",
            self.cfg.device_name, self.cfg.device_addr
        );
        if self.cfg.device_addr.is_some() {
            self.state
                .push_log(format!(">> Scanning for {}…", self.cfg.device_name));
        } else {
            self.state.push_log(">> Scanning for BLE devices…");
        }
        self.spawn_scan();
    }

    async fn handle_ble_event(&mut self, evt: BleEvent) {
        match evt {
            BleEvent::Connected => {
                info!("run: BleEvent::Connected received");
                self.state.ble_connected = true;
                self.state.mode = AppMode::Session;
                self.state.push_log(">> BLE connected — belt ready");
            }
            BleEvent::Disconnected => self.handle_disconnect().await,
            BleEvent::Status(st) => self.handle_status(st),
            BleEvent::StepCount(steps) => {
                debug!("run: BleEvent::StepCount steps={steps}");
                self.state.stats.total_steps = steps;
            }
            BleEvent::MachineStatus(ms) => self.handle_machine_status(ms),
            BleEvent::DeviceFound(dev) => self.handle_device_found(dev).await,
            BleEvent::Error(msg) => {
                error!("run: BleEvent::Error: {msg}");
                self.state.push_log(format!("!! {msg}"));
            }
        }
    }

    async fn handle_key(&mut self, code: KeyCode) {
        // Config dialog gets first priority over all other key handlers
        if self.state.config_dialog.visible {
            self.handle_config_dialog_key(code);
            return;
        }

        // Speed-input popup intercepts keys while visible
        if self.state.speed_input.visible {
            self.handle_speed_input_key(code).await;
            return;
        }

        // [c] opens the config dialog from any mode except Connecting
        if code == KeyCode::Char('c') && self.state.mode != AppMode::Connecting {
            self.open_config_dialog();
            return;
        }

        match self.state.mode {
            AppMode::Scanning => self.handle_scanning_key(code).await,
            AppMode::Connecting => {
                if code == KeyCode::Char('r') {
                    self.state.push_log(">> Aborting — back to scan…");
                    self.state.mode = AppMode::Scanning;
                    self.state.connecting_addr = None;
                    self.reset_cmd_channel();
                    self.start_scan().await;
                }
            }
            AppMode::SessionSummary => self.handle_summary_key(code).await,
            AppMode::Session => self.handle_session_key(code).await,
        }
    }

    fn open_config_dialog(&mut self) {
        let device_display = match &self.cfg.device_addr {
            Some(addr) => format!("{} ({})", self.cfg.device_name, addr),
            None => format!("{} — no saved address", self.cfg.device_name),
        };
        self.state.open_config_dialog(
            device_display,
            self.cfg.fit_client_id.clone().unwrap_or_default(),
            self.cfg.fit_client_secret.clone().unwrap_or_default(),
            self.cfg.fit_refresh_token.is_some(),
        );
    }

    fn handle_config_dialog_key(&mut self, code: KeyCode) {
        let n = CONFIG_DIALOG_ITEMS;
        match code {
            KeyCode::Esc => {
                self.state.config_dialog.visible = false;
            }
            KeyCode::Tab | KeyCode::Down => {
                self.state.config_dialog.selected = (self.state.config_dialog.selected + 1) % n;
            }
            KeyCode::Up => {
                self.state.config_dialog.selected = (self.state.config_dialog.selected + n - 1) % n;
            }
            KeyCode::Backspace => match self.state.config_dialog.selected {
                1 => {
                    self.state.config_dialog.input_client_id.pop();
                }
                2 => {
                    self.state.config_dialog.input_client_secret.pop();
                }
                _ => {}
            },
            KeyCode::Enter => {
                let sel = self.state.config_dialog.selected;
                match sel {
                    0 => self.do_unpair(),
                    1 | 2 => {
                        self.state.config_dialog.selected = (sel + 1) % n;
                    }
                    3 => self.do_save_fit_config(),
                    _ => {}
                }
            }
            KeyCode::Char(c) => match self.state.config_dialog.selected {
                1 => self.state.config_dialog.input_client_id.push(c),
                2 => self.state.config_dialog.input_client_secret.push(c),
                _ => {}
            },
            _ => {}
        }
    }

    async fn handle_speed_input_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.state.speed_input = Default::default();
            }
            KeyCode::Backspace => {
                self.state.speed_input.digits.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() && self.state.speed_input.digits.len() < 2 => {
                self.state.speed_input.digits.push(c);
            }
            KeyCode::Enter => {
                if let Some(raw) = self.state.speed_input.parse_raw() {
                    let clamped = raw.clamp(self.state.min_speed_raw, self.state.max_speed_raw);
                    let _ = self.cmd_tx.send(Cmd::SetSpeed(clamped)).await;
                    self.state
                        .push_log(format!(">> Set speed {:.1} km/h", clamped as f32 / 10.0));
                }
                self.state.speed_input = Default::default();
            }
            _ => {}
        }
    }

    fn do_unpair(&mut self) {
        self.cfg.device_addr = None;
        match self.cfg.save() {
            Ok(()) => self.state.push_log(">> Device unpaired"),
            Err(e) => self.state.push_log(format!("!! Config save failed: {e}")),
        }
        self.state.config_dialog.visible = false;
    }

    fn do_save_fit_config(&mut self) {
        let id = self.state.config_dialog.input_client_id.trim().to_string();
        let secret = self
            .state
            .config_dialog
            .input_client_secret
            .trim()
            .to_string();
        self.cfg.fit_client_id = if id.is_empty() { None } else { Some(id) };
        self.cfg.fit_client_secret = if secret.is_empty() {
            None
        } else {
            Some(secret)
        };
        match self.cfg.save() {
            Ok(()) => {
                self.state.fit_client_id = self.cfg.fit_client_id.clone();
                self.state.fit_client_secret = self.cfg.fit_client_secret.clone();
                self.state.push_log(">> Google Fit config saved");
            }
            Err(e) => self.state.push_log(format!("!! Config save failed: {e}")),
        }
        self.state.config_dialog.visible = false;
    }

    fn handle_status(&mut self, st: protocol::StatusPacket) {
        debug!(
            "run: BleEvent::Status speed={} dist={} time={}s kcal={:?}",
            st.speed_raw, st.dist_raw, st.time_secs, st.kcal
        );
        let was_session = self.state.session;
        self.state.last_belt_state = Some(st.belt_state);
        self.state.stats.update(st.dist_raw, st.steps, st.speed_raw);
        if st.time_secs > 0 {
            self.state.stats.active_seconds = st.time_secs;
        }
        if let Some(kcal) = st.kcal {
            self.state.stats.calories = kcal as f32;
        }
        self.state
            .sync_session_to_belt(st.belt_state, std::time::SystemTime::now());
        if was_session != self.state.session {
            match self.state.session {
                SessionState::Running => self.state.push_log(">> Belt running"),
                SessionState::Paused => self.state.push_log(">> Belt stopped"),
                SessionState::Idle => {}
            }
        }
    }

    fn handle_machine_status(&mut self, ms: protocol::MachineStatus) {
        use protocol::MachineStatus::*;

        info!("run: BleEvent::MachineStatus {ms:?}");
        match ms {
            StoppedBySafetyKey => {
                self.state.push_log("!! Belt stopped by safety key");
                self.state
                    .sync_session_to_belt(protocol::BELT_STOPPED, std::time::SystemTime::now());
            }
            StoppedByUser => {
                self.state.push_log(">> Belt stopped from device panel");
                self.state
                    .sync_session_to_belt(protocol::BELT_STOPPED, std::time::SystemTime::now());
            }
            StartedOrResumed => {
                self.state.push_log(">> Belt started from device panel");
                self.state
                    .sync_session_to_belt(protocol::BELT_RUNNING, std::time::SystemTime::now());
            }
            SpeedChanged(raw) => {
                let kmh = raw as f32 / 100.0;
                self.state
                    .push_log(format!(">> Speed changed to {kmh:.1} km/h from device"));
            }
            ControlPermissionLost => {
                self.state
                    .push_log("!! FTMS control lost — reconnect may be needed");
            }
            Reset => {
                self.state.push_log(">> Device reset");
            }
            Unknown(code) => {
                debug!("run: unknown machine status code {code:#04x}");
            }
        }
    }

    async fn handle_device_found(&mut self, dev: types::DiscoveredDevice) {
        let dev_name = dev.name.clone();
        let dev_addr = dev.addr.clone();
        debug!("run: BleEvent::DeviceFound name={dev_name:?} addr={dev_addr}");
        self.state.add_scan_result(dev);

        if self.state.mode == AppMode::Scanning
            && self.cfg.device_addr.is_some()
            && matches_saved_device(
                &self.cfg.device_name,
                self.cfg.device_addr.as_deref(),
                &dev_name,
                &dev_addr,
            )
        {
            info!("run: auto-connecting to saved device name={dev_name:?} addr={dev_addr}");
            self.begin_connection(dev_name.clone(), dev_addr.clone(), true);
            if self.cfg.device_addr.as_deref() != Some(dev_addr.as_str()) {
                if let Err(e) = self.cfg.save_selected_device(&dev_addr, &dev_name) {
                    self.state.push_log(format!("!! Config save failed: {e}"));
                } else {
                    self.state.push_log(">> Updated saved address");
                }
            }
            self.spawn_session_task(dev_name, Some(dev_addr));
        }
    }

    async fn handle_scanning_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.state.scan_cursor_up(),
            KeyCode::Down | KeyCode::Char('j') => self.state.scan_cursor_down(),
            KeyCode::Char('r') => self.start_scan().await,
            KeyCode::Enter => {
                if let Some(dev) = self.state.selected_scan_device() {
                    let addr = dev.addr.clone();
                    let name = dev.name.clone();
                    self.begin_connection(name.clone(), addr.clone(), false);
                    if let Err(e) = self.cfg.save_selected_device(&addr, &name) {
                        self.state.push_log(format!("!! Config save failed: {e}"));
                    } else {
                        self.state.push_log(">> Saved to padctl.toml");
                    }
                    self.spawn_session_task(name, Some(addr));
                }
            }
            _ => {}
        }
    }

    async fn handle_summary_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('y') if self.state.fit_configured() => {
                if let Some(summary) = self.state.summary.take() {
                    self.state.push_log(">> Syncing to Google Fit…");
                    fit::sync_in_background(self.cfg.clone(), summary, self.fit_log_tx.clone());
                }
                self.state.mode = AppMode::Session;
                self.state.session_start_at = None;
            }
            KeyCode::Char('n') | KeyCode::Char('x') | KeyCode::Char('q') => {
                self.state.summary = None;
                self.state.session_start_at = None;
                self.state.mode = AppMode::Session;
            }
            _ => {}
        }
    }

    async fn handle_session_key(&mut self, code: KeyCode) {
        match (&self.state.session, code) {
            (SessionState::Idle, KeyCode::Char('s')) => {
                if !self.state.ble_connected {
                    self.state.push_log("!! Not connected");
                    return;
                }
                let belt_already_running =
                    self.state.last_belt_state == Some(protocol::BELT_RUNNING);
                self.state.stats.reset();
                self.state.session_start_at = Some(std::time::SystemTime::now());
                if belt_already_running {
                    self.state
                        .sync_session_to_belt(protocol::BELT_RUNNING, std::time::SystemTime::now());
                    self.state
                        .push_log(">> Belt already running — session started");
                } else {
                    let _ = self.cmd_tx.send(Cmd::SetMode(protocol::MODE_MANUAL)).await;
                    self.state.push_log(">> Starting belt…");
                    let _ = self.cmd_tx.send(Cmd::StartBelt).await;
                }
            }
            (SessionState::Running, KeyCode::Char('p')) => {
                self.state.resume_speed_raw = (self.state.stats.current_speed_kmh * 10.0) as u8;
                let _ = self.cmd_tx.send(Cmd::StopBelt).await;
                self.state.push_log(">> Pausing belt…");
            }
            (SessionState::Running, KeyCode::Up | KeyCode::Char('k')) => {
                let now = std::time::Instant::now();
                let double = self
                    .state
                    .last_up_at
                    .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_WINDOW);
                let step = if double {
                    5u8
                } else {
                    self.state.speed_step_raw
                };
                self.state.last_up_at = if double { None } else { Some(now) };
                let cur = (self.state.stats.current_speed_kmh * 10.0) as u8;
                let new = (cur + step).min(self.state.max_speed_raw);
                let _ = self.cmd_tx.send(Cmd::SetSpeed(new)).await;
            }
            (SessionState::Running, KeyCode::Down | KeyCode::Char('j')) => {
                let now = std::time::Instant::now();
                let double = self
                    .state
                    .last_down_at
                    .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_WINDOW);
                let step = if double {
                    5u8
                } else {
                    self.state.speed_step_raw
                };
                self.state.last_down_at = if double { None } else { Some(now) };
                let cur = (self.state.stats.current_speed_kmh * 10.0) as u8;
                let new = cur.saturating_sub(step).max(self.state.min_speed_raw);
                let _ = self.cmd_tx.send(Cmd::SetSpeed(new)).await;
            }
            (SessionState::Running, KeyCode::Char('h')) => {
                let _ = self
                    .cmd_tx
                    .send(Cmd::SetSpeed(self.state.min_speed_raw))
                    .await;
            }
            (SessionState::Running, KeyCode::Char('l')) => {
                let _ = self
                    .cmd_tx
                    .send(Cmd::SetSpeed(self.state.max_speed_raw))
                    .await;
            }
            (SessionState::Running, KeyCode::Char('g')) => {
                self.state.speed_input.visible = true;
                self.state.speed_input.digits.clear();
            }
            (SessionState::Paused, KeyCode::Char('r')) => {
                let _ = self.cmd_tx.send(Cmd::StartBelt).await;
                time::sleep(Duration::from_millis(300)).await;
                let _ = self
                    .cmd_tx
                    .send(Cmd::SetSpeed(self.state.resume_speed_raw))
                    .await;
                self.state.push_log(">> Resuming belt…");
            }
            (SessionState::Paused, KeyCode::Char('x')) => {
                let ended_at = std::time::SystemTime::now();
                let started_at = self.state.session_start_at.unwrap_or(ended_at);
                self.state.summary = Some(SessionSummary {
                    duration_secs: self.state.stats.active_seconds,
                    steps: self.state.stats.total_steps,
                    distance_km: self.state.stats.total_distance_km,
                    calories: self.state.stats.calories,
                    started_at,
                    ended_at,
                });
                self.state.apply_session_event(SessionEvent::Stop);
                self.state.mode = AppMode::SessionSummary;
                self.state.push_log(">> Session ended");
            }
            _ => {}
        }
    }

    async fn handle_disconnect(&mut self) {
        let was_connected = self.state.ble_connected;
        self.state.ble_connected = false;
        warn!(
            "run: BleEvent::Disconnected (mode={:?} was_connected={was_connected})",
            self.state.mode
        );

        match (self.state.mode, was_connected) {
            (AppMode::Session, true) => {
                self.state.push_log(">> BLE dropped — reconnecting…");
                self.reset_cmd_channel();
                self.spawn_session_task(self.cfg.device_name.clone(), self.cfg.device_addr.clone());
            }
            (AppMode::Session, false) => {
                self.state.push_log(">> Reconnect failed — back to scan");
                self.state.mode = AppMode::Scanning;
                self.state.session = SessionState::Idle;
                self.reset_cmd_channel();
                self.start_scan().await;
            }
            (AppMode::Connecting, _) => {
                self.state
                    .push_log(">> Connection failed — press [r] to rescan");
                self.state.mode = AppMode::Scanning;
                self.reset_cmd_channel();
                self.start_scan().await;
            }
            _ => {
                self.state.push_log(">> BLE disconnected");
            }
        }
    }

    async fn start_scan(&mut self) {
        self.state.clear_scan_results();
        self.state.push_log(">> Scanning for BLE devices…");
        self.spawn_scan();
    }

    fn spawn_scan(&mut self) {
        self.stop_scan();
        let (new_stop_tx, new_stop_rx) = watch::channel(false);
        self.scan_stop_tx = Some(new_stop_tx);

        let etx = self.evt_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = scan_devices(etx.clone(), new_stop_rx).await {
                let _ = etx.send(BleEvent::Error(format!("Scan error: {e}"))).await;
            }
        });
    }

    fn stop_scan(&mut self) {
        if let Some(tx) = self.scan_stop_tx.take() {
            let _ = tx.send(true);
        }
    }

    fn reset_cmd_channel(&mut self) {
        let (new_tx, new_rx) = mpsc::channel::<Cmd>(32);
        self.cmd_tx = new_tx;
        self.cmd_rx_slot = Some(new_rx);
    }

    fn begin_connection(&mut self, name: String, addr: String, auto: bool) {
        self.state.mode = AppMode::Connecting;
        self.state.connecting_addr = Some(addr.clone());
        self.state.device_name = name.clone();
        if auto {
            self.state
                .push_log(format!(">> Auto-connecting to {name}…"));
        } else {
            self.state
                .push_log(format!(">> Connecting to {name} ({addr})…"));
        }
        self.stop_scan();
    }

    fn spawn_session_task(&mut self, device_name: String, device_addr: Option<String>) {
        if let Some(rx) = self.cmd_rx_slot.take() {
            let etx = self.evt_tx.clone();
            let scan_timeout = self.scan_timeout;
            tokio::spawn(async move {
                ble_session_task(device_name, device_addr, scan_timeout, rx, etx).await;
            });
        }
    }
}

/// Long-running BLE session task: connects, streams notifications, handles commands.
async fn ble_session_task(
    device_name: String,
    device_addr: Option<String>,
    scan_timeout: u64,
    mut cmd_rx: mpsc::Receiver<Cmd>,
    evt_tx: mpsc::Sender<BleEvent>,
) {
    info!(
        "ble_session_task: starting — name={device_name:?} addr={device_addr:?} timeout={scan_timeout}s"
    );
    let mut driver = BleDriver::new(device_name, device_addr, scan_timeout);

    info!("ble_session_task: calling driver.connect()…");
    match driver.connect().await {
        Ok(()) => {
            info!("ble_session_task: driver.connect() Ok — sending Connected event");
            let _ = evt_tx.send(BleEvent::Connected).await;
        }
        Err(e) => {
            error!("ble_session_task: driver.connect() failed: {e}");
            let _ = evt_tx
                .send(BleEvent::Error(format!("Connect failed: {e}")))
                .await;
            let _ = evt_tx.send(BleEvent::Disconnected).await;
            return;
        }
    }

    // Forward BLE notifications → BleEvent::Status
    // Filter by characteristic UUID: only Treadmill Data (0x2ACD) is parsed;
    // Control Point indications (0x2AD9) are logged and discarded.
    info!("ble_session_task: setting up notification stream…");
    let treadmill_uuid = Uuid::parse_str(TREADMILL_DATA_UUID).unwrap();
    let proprietary_uuid = Uuid::parse_str(PROPRIETARY_NOTIFY_UUID).unwrap();
    let machine_status_uuid = Uuid::parse_str(FITNESS_MACHINE_STATUS_UUID).unwrap();
    match driver.get_notifications().await {
        Ok(notif_stream) => {
            info!("ble_session_task: notification stream ready, spawning forwarder");
            let notif_tx = evt_tx.clone();
            tokio::spawn(async move {
                futures::pin_mut!(notif_stream);
                let mut count = 0u64;
                while let Some(notif) = notif_stream.next().await {
                    debug!(
                        "notif: uuid={} {} B: {:02X?}",
                        notif.uuid,
                        notif.value.len(),
                        notif.value
                    );

                    if notif.uuid == treadmill_uuid {
                        // FTMS Treadmill Data → speed, distance, elapsed time
                        match protocol::parse_ftms_treadmill_data(&notif.value) {
                            Ok(status) => {
                                count += 1;
                                debug!(
                                    "notif: FTMS #{count} speed={} dist={} time={}s",
                                    status.speed_raw, status.dist_raw, status.time_secs
                                );
                                let _ = notif_tx.send(BleEvent::Status(status)).await;
                            }
                            Err(e) => {
                                debug!("notif: parse_ftms_treadmill_data failed: {e}");
                            }
                        }
                    } else if notif.uuid == machine_status_uuid {
                        // FTMS Fitness Machine Status (0x2ADA) — device-initiated events
                        let ms = protocol::parse_machine_status(&notif.value);
                        debug!("notif: machine status {:?} raw={:02X?}", ms, notif.value);
                        let _ = notif_tx.send(BleEvent::MachineStatus(ms)).await;
                    } else if notif.uuid == proprietary_uuid {
                        // Proprietary 0xfff1 → F7 A2 status packet
                        match protocol::parse_status(&notif.value) {
                            Ok(status) => {
                                // Log all decoded fields + the unknown tail (bytes 14–19)
                                let tail = if notif.value.len() > 14 {
                                    &notif.value[14..]
                                } else {
                                    &[]
                                };
                                debug!(
                                    "notif: proprietary belt={:#04x} speed={} time={}s dist={} steps={} tail={:02X?}",
                                    status.belt_state, status.speed_raw, status.time_secs,
                                    status.dist_raw, status.steps, tail
                                );
                                let _ = notif_tx.send(BleEvent::StepCount(status.steps)).await;
                            }
                            Err(e) => {
                                debug!(
                                    "notif: proprietary parse failed: {e} — raw: {:02X?}",
                                    notif.value
                                );
                            }
                        }
                    } else {
                        debug!("notif: skipping unknown uuid={}", notif.uuid);
                    }
                }
                warn!("notif: stream ended after {count} notifications — device disconnected");
                let _ = notif_tx.send(BleEvent::Disconnected).await;
            });
        }
        Err(e) => {
            error!("ble_session_task: get_notifications failed: {e}");
            let _ = evt_tx
                .send(BleEvent::Error(format!("Notification stream error: {e}")))
                .await;
        }
    }

    // Periodically poll the proprietary 0xfff2 characteristic to trigger
    // step-count responses on 0xfff1 (device only sends on request).
    let mut poll_tick = time::interval(Duration::from_secs(1));
    poll_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    // The device silently drops the BLE connection after ~15 min if no FTMS
    // Control Point command arrives.  Re-sending RequestControl every 5 min
    // keeps the control session alive without changing any belt state.
    let mut keepalive_tick = time::interval_at(
        time::Instant::now() + Duration::from_secs(300),
        Duration::from_secs(300),
    );
    keepalive_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    // Handle commands from the TUI — FTMS encoding
    info!("ble_session_task: entering command loop");
    let mut consecutive_poll_failures: u32 = 0;
    loop {
        let cmd = tokio::select! {
            _ = poll_tick.tick() => {
                match driver.poll_proprietary_stats().await {
                    Ok(()) => { consecutive_poll_failures = 0; }
                    Err(e) => {
                        consecutive_poll_failures += 1;
                        debug!("ble_session_task: poll_proprietary_stats error: {e}");
                        if consecutive_poll_failures >= 3 {
                            warn!("ble_session_task: {} consecutive poll failures — device disconnected", consecutive_poll_failures);
                            let _ = evt_tx.send(BleEvent::Disconnected).await;
                            break;
                        }
                    }
                }
                continue;
            }
            _ = keepalive_tick.tick() => {
                info!("ble_session_task: FTMS keep-alive — RequestControl [0x00]");
                if let Err(e) = driver.write_command(&[0x00]).await {
                    warn!("ble_session_task: keep-alive write failed — device disconnected: {e}");
                    let _ = evt_tx.send(BleEvent::Disconnected).await;
                    break;
                }
                continue;
            }
            maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                Some(c) => c,
                None => break,
            },
        };
        let res: crate::error::Result<()> = match cmd {
            Cmd::StartBelt => {
                // FTMS: Start or Resume (0x07)
                info!("ble_session_task: cmd StartBelt → FTMS Start/Resume [0x07]");
                driver.write_command(&[0x07]).await
            }
            Cmd::StopBelt => {
                // FTMS: Stop (0x08, param 0x01 = stop)
                info!("ble_session_task: cmd StopBelt → FTMS Stop [0x08 0x01]");
                driver.write_command(&[0x08, 0x01]).await
            }
            Cmd::SetSpeed(raw) => {
                // raw is in 0.1 km/h units (e.g. 30 = 3.0 km/h)
                // FTMS SetTargetSpeed (0x02) expects uint16 LE in 0.01 km/h → raw * 10
                let ftms_speed = (raw as u16) * 10;
                info!("ble_session_task: cmd SetSpeed({raw}) → FTMS [0x02 {ftms_speed:#06X}]");
                driver
                    .write_command(&[0x02, (ftms_speed & 0xFF) as u8, (ftms_speed >> 8) as u8])
                    .await
            }
            Cmd::SetMode(m) => {
                // SetMode not used in FTMS — silently ignore
                debug!("ble_session_task: SetMode({m}) ignored (FTMS device)");
                Ok(())
            }
            Cmd::Quit => {
                info!("ble_session_task: cmd Quit — disconnecting");
                let _ = driver.disconnect().await;
                break;
            }
        };
        if let Err(e) = res {
            error!("ble_session_task: BLE write error: {e}");
            let _ = evt_tx
                .send(BleEvent::Error(format!("BLE write error: {e}")))
                .await;
        }
    }
    info!("ble_session_task: exiting");
}
