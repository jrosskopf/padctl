use std::f64::consts::{PI, TAU};

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Circle, Context},
        Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap,
    },
    Frame,
};

use crate::session::SessionState;
use crate::ui::app::{AppMode, AppState, ConfigDialog, SessionSummary, SpeedInput};

// ─── Palette (btop-inspired, cypherpunk green core) ───────────────────────────

/// Rich seafoam-green — primary text / borders
const C_PRIMARY: Color = Color::Rgb(0, 230, 160);
/// Dimmed version for labels, secondary borders
const C_DIM: Color = Color::Rgb(0, 110, 75);
/// Very dark background
const C_BG: Color = Color::Rgb(8, 8, 12);
/// Amber — paused state, warnings
const C_AMBER: Color = Color::Rgb(255, 185, 0);
/// Red — disconnected, errors
const C_RED: Color = Color::Rgb(220, 60, 60);
/// Cyan accent — disc ring, highlights
const C_CYAN: Color = Color::Rgb(0, 210, 220);
/// Disc sweep arc colour (bright lime)
const C_SWEEP: Color = Color::Rgb(120, 255, 80);
/// Pump pulse colour (hot pink / magenta flash)
const C_PUMP: Color = Color::Rgb(255, 80, 200);
/// Speed bar fill gradient start (green)
const C_BAR_LO: Color = Color::Rgb(40, 200, 100);
/// Speed bar fill gradient end (amber, shown near max speed)
const C_BAR_HI: Color = Color::Rgb(230, 180, 0);

// ─── Public entry point ───────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, state: &AppState) {
    let area = f.area();
    match state.mode {
        AppMode::Scanning => draw_scanning(f, state),
        AppMode::Connecting => draw_connecting(f, state),
        AppMode::Session => draw_session(f, state),
        AppMode::SessionSummary => draw_summary(f, state),
    }
    if state.config_dialog.visible {
        draw_config_dialog(f, &state.config_dialog, area);
    }
    if state.speed_input.visible {
        draw_speed_input(f, &state.speed_input, area);
    }
}

// ─── Config dialog (modal overlay) ───────────────────────────────────────────

fn draw_config_dialog(f: &mut Frame, dlg: &ConfigDialog, screen: Rect) {
    const W: u16 = 64;
    const H: u16 = 17;
    let w = W.min(screen.width);
    let h = H.min(screen.height);
    let x = screen.x + screen.width.saturating_sub(w) / 2;
    let y = screen.y + screen.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup);

    let sel = dlg.selected;
    let inner_w = w.saturating_sub(2) as usize;
    let field_w = 26usize;

    let arrow = |focused: bool| if focused { "▶" } else { " " };
    let btn = |focused: bool| -> Style {
        if focused {
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_DIM)
        }
    };

    // Text-field helpers
    let render_field = |buf: &str, placeholder: &str, mask: bool, focused: bool| -> Line {
        let val: String = if buf.is_empty() {
            placeholder.to_string()
        } else if mask {
            "●".repeat(buf.chars().count())
        } else {
            // show only the trailing `field_w` chars when it overflows
            if buf.chars().count() > field_w {
                buf.chars().skip(buf.chars().count() - field_w).collect()
            } else {
                buf.to_string()
            }
        };
        let padded = format!("{:<field_w$}", val);
        let cursor = if focused { "█" } else { " " };
        let style = if focused {
            Style::default().fg(C_PRIMARY)
        } else {
            Style::default().fg(C_DIM)
        };
        Span::styled(format!("[{padded}{cursor}]"), style).into()
    };

    let id_field = render_field(&dlg.input_client_id, "— not set —", false, sel == 1);
    let sec_field = render_field(&dlg.input_client_secret, "— not set —", true, sel == 2);

    let device_line = truncate(&dlg.device_display, inner_w.saturating_sub(2));
    let sep = "─".repeat(inner_w);

    let token_txt = if dlg.fit_token_saved {
        ("  Refresh token: saved ✓", C_PRIMARY)
    } else {
        ("  Refresh token: not yet saved", C_DIM)
    };

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled("  ─── DEVICE ", Style::default().fg(C_DIM))),
        Line::from(Span::styled(
            format!("  {device_line}"),
            Style::default().fg(C_PRIMARY),
        )),
        Line::from(vec![
            Span::styled(format!("  {} ", arrow(sel == 0)), btn(sel == 0)),
            Span::styled("[ Unpair — forget saved device ]", btn(sel == 0)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  ─── GOOGLE FIT ",
            Style::default().fg(C_DIM),
        )),
        // Client ID row: label + field on same line
        {
            let mut spans = vec![Span::styled(
                "  Client ID      ",
                Style::default().fg(C_DIM),
            )];
            spans.extend(id_field.spans);
            Line::from(spans)
        },
        // Client Secret row
        {
            let mut spans = vec![Span::styled(
                "  Client Secret  ",
                Style::default().fg(C_DIM),
            )];
            spans.extend(sec_field.spans);
            Line::from(spans)
        },
        Line::from(Span::styled(token_txt.0, Style::default().fg(token_txt.1))),
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("  {} ", arrow(sel == 3)), btn(sel == 3)),
            Span::styled("[ Save Fit config ]", btn(sel == 3)),
        ]),
        Line::from(""),
        Line::from(Span::styled(sep, Style::default().fg(C_DIM))),
        Line::from(Span::styled(
            "  [Esc] close   [Tab/↓↑] navigate   [Enter] activate",
            Style::default().fg(C_DIM),
        )),
    ];

    let dialog = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(C_PRIMARY))
            .title(Span::styled(
                " CONFIG ",
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(C_BG)),
    );
    f.render_widget(dialog, popup);
}

// ─── Speed-input popup (modal overlay) ───────────────────────────────────────

fn draw_speed_input(f: &mut Frame, si: &SpeedInput, screen: Rect) {
    const W: u16 = 40;
    const H: u16 = 9;
    let w = W.min(screen.width);
    let h = H.min(screen.height);
    let x = screen.x + screen.width.saturating_sub(w) / 2;
    let y = screen.y + screen.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup);

    let typed = if si.digits.is_empty() {
        "__".to_string()
    } else {
        format!("{:_<2}", si.digits)
    };
    let preview = match si.parse_raw() {
        Some(raw) => format!("{:.1} km/h", raw as f32 / 10.0),
        None => "— —".to_string(),
    };

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("   Enter speed  ", Style::default().fg(C_DIM)),
            Span::styled(
                typed,
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  →  ", Style::default().fg(C_DIM)),
            Span::styled(
                preview,
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "   raw tenths · range 10–60 (1.0–6.0 km/h)",
            Style::default().fg(C_DIM),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "   [0-9] digits   [Enter] confirm   [Esc] cancel",
            Style::default().fg(C_DIM),
        )),
    ];

    let dialog = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(C_PRIMARY))
            .title(Span::styled(
                " SET SPEED ",
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(C_BG)),
    );
    f.render_widget(dialog, popup);
}

// ─── Scanning screen ──────────────────────────────────────────────────────────

fn draw_scanning(f: &mut Frame, state: &AppState) {
    let area = f.area();

    let show_log = !state.log_lines.is_empty();
    let constraints: Vec<Constraint> = if show_log {
        vec![
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(4),
            Constraint::Length(3),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(3),
        ]
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    f.render_widget(
        header_bar(
            " ▓ PADCTL ",
            &format!("[SCANNING ◌]  :: {}", state.device_name),
            C_AMBER,
        ),
        layout[0],
    );

    // Device list
    let items: Vec<ListItem> = if state.scan_results.is_empty() {
        vec![ListItem::new(Span::styled(
            "  Scanning… (no devices found yet)",
            Style::default().fg(C_DIM),
        ))]
    } else {
        state
            .scan_results
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let cursor = if i == state.scan_cursor { "▶ " } else { "  " };
                let rssi_str = d
                    .rssi
                    .map(|r| format!("{r:4} dBm"))
                    .unwrap_or_else(|| "    ---".into());
                let name_col = format!("{:<24}", truncate(&d.name, 24));
                let text = format!("{cursor}{name_col}  {}  {rssi_str}", d.addr);
                let style = if i == state.scan_cursor {
                    Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_DIM)
                };
                ListItem::new(text).style(style)
            })
            .collect()
    };

    let list_title = format!(
        " DEVICES ({}) ",
        if state.scan_results.is_empty() {
            "searching…".into()
        } else {
            format!("{} found", state.scan_results.len())
        }
    );

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(C_PRIMARY))
            .title(Span::styled(
                list_title,
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(C_BG)),
    );
    f.render_widget(list, layout[1]);

    if show_log {
        draw_log(f, state, layout[2]);
        f.render_widget(
            keys("[↑/k] up  [↓/j] down  [Enter] connect  [r] rescan  [c] config  [q] quit"),
            layout[3],
        );
    } else {
        f.render_widget(
            keys("[↑/k] up  [↓/j] down  [Enter] connect  [r] rescan  [c] config  [q] quit"),
            layout[2],
        );
    }
}

// ─── Connecting screen ────────────────────────────────────────────────────────

fn draw_connecting(f: &mut Frame, state: &AppState) {
    let area = f.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(area);

    f.render_widget(
        header_bar(" ▓ PADCTL ", "[CONNECTING ◌]", C_AMBER),
        layout[0],
    );

    let addr_line = state
        .connecting_addr
        .as_deref()
        .unwrap_or("scanning by name…");
    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Connecting to {}…", state.device_name),
            Style::default().fg(C_PRIMARY),
        )),
        Line::from(Span::styled(
            format!("  {addr_line}"),
            Style::default().fg(C_DIM),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(C_DIM))
            .style(Style::default().bg(C_BG)),
    );
    f.render_widget(body, layout[1]);

    draw_log(f, state, layout[2]);
    f.render_widget(keys("[q] quit  [r] back to scan"), layout[3]);
}

// ─── Session summary screen ───────────────────────────────────────────────────

fn draw_summary(f: &mut Frame, state: &AppState) {
    let area = f.area();
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Length(6), // stat grid
            Constraint::Min(3),    // sync prompt
            Constraint::Length(3), // keys
        ])
        .split(area);

    f.render_widget(
        header_bar(" ▓ PADCTL ▓ ", "SESSION COMPLETE", C_PRIMARY),
        root[0],
    );

    // ── Stats grid ────────────────────────────────────────────────────────
    let summary_ref: Option<&SessionSummary> = state.summary.as_ref(); // snapshot from session end

    let (time_str, steps_str, dist_str, kcal_str) = match summary_ref {
        Some(s) => (
            AppState::format_time(s.duration_secs),
            format!("{}", s.steps),
            format!("{:.2} km", s.distance_km),
            format!("{:.0}", s.calories),
        ),
        None => ("—".into(), "—".into(), "—".into(), "—".into()),
    };

    let stat_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
        ])
        .split(root[1]);

    let stat_cells = [
        ("TIME", time_str, C_CYAN),
        ("STEPS", steps_str, C_PRIMARY),
        ("DISTANCE", dist_str, C_PRIMARY),
        ("KCAL", kcal_str, C_AMBER),
    ];

    for (i, (label, value, color)) in stat_cells.iter().enumerate() {
        let p = Paragraph::new(vec![
            Line::from(Span::styled(*label, Style::default().fg(C_DIM))),
            Line::from(Span::styled(
                value.as_str(),
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(C_DIM))
                .style(Style::default().bg(C_BG)),
        )
        .alignment(Alignment::Center);
        f.render_widget(p, stat_cols[i]);
    }

    // ── Sync prompt ───────────────────────────────────────────────────────
    let prompt_lines: Vec<Line> = if state.fit_configured() {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Sync this session to Google Fit?",
                Style::default().fg(C_PRIMARY),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "  [y] ",
                    Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "SYNC",
                    Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
                ),
                Span::styled("      ", Style::default().fg(C_DIM)),
                Span::styled("[n] ", Style::default().fg(C_DIM)),
                Span::styled("DISCARD", Style::default().fg(C_DIM)),
            ]),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Configure fit_client_id / fit_client_secret in padctl.toml to enable Fit sync.",
                Style::default().fg(C_DIM),
            )),
        ]
    };

    let prompt = Paragraph::new(prompt_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(C_DIM))
            .style(Style::default().bg(C_BG)),
    );
    f.render_widget(prompt, root[2]);

    let kb = if state.fit_configured() {
        "[y] SYNC   [n] DISCARD   [c] CONFIG   [q] QUIT"
    } else {
        "[n] DONE   [c] CONFIG   [q] QUIT"
    };
    f.render_widget(keys(kb), root[3]);
}

// ─── Session screen ───────────────────────────────────────────────────────────

fn draw_session(f: &mut Frame, state: &AppState) {
    let area = f.area();

    // Layout:
    //   header  (3)
    //   ┌─disc(44w×24h)─┬─stats(rest×24h)─┐  (24)
    //   └───────────────┴─────────────────┘
    //   gauge           (3)
    //   log             (min 4)
    //   keys            (3)
    //
    // The disc area is 44×24, which at 2:1 cell aspect (8×16px) gives
    // an effective 42w×22h chars ≈ 336×352px → physically square. ✓
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Length(24), // disc + stats row
            Constraint::Length(3),  // gauge
            Constraint::Min(4),     // log
            Constraint::Length(3),  // keys
        ])
        .split(area);

    // Header
    let ble_label = if state.ble_connected {
        "BLE ●"
    } else {
        "BLE ✗"
    };
    let ble_clr = if state.ble_connected { C_CYAN } else { C_RED };
    let sess_clr = session_color(&state.session);

    let hdr = Paragraph::new(Line::from(vec![
        Span::styled(
            " ▓ PADCTL ▓ ",
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{}]", state.status_label()),
            Style::default().fg(sess_clr).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(C_DIM)),
        Span::styled(
            ble_label,
            Style::default().fg(ble_clr).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}  ", state.device_name),
            Style::default().fg(C_DIM),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(C_PRIMARY))
            .style(Style::default().bg(C_BG)),
    )
    .alignment(Alignment::Left);
    f.render_widget(hdr, root[0]);

    // Disc + stats + clock side by side (all 24 rows tall)
    let disc_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(44), // disc (44w × 24h ≈ 336×352 px — physically square)
            Constraint::Min(30),    // stats grid
            Constraint::Length(44), // clock (mirrors disc on the right)
        ])
        .split(root[1]);

    draw_disc(f, state, disc_row[0]);
    draw_stats(f, state, disc_row[1]);
    draw_clock(f, state, disc_row[2]);

    draw_gauge(f, state, root[2]);
    draw_log(f, state, root[3]);

    let kb = match state.session {
        SessionState::Idle    => "[s] START   [c] CONFIG   [q] QUIT",
        SessionState::Running => "[p] PAUSE   [↑/k] +0.1   [↓/j] -0.1   [h] SLOW   [l] MAX   [g] SET   [c] CONFIG   [q] QUIT",
        SessionState::Paused  => "[r] RESUME   [x] END SESSION   [c] CONFIG   [q] QUIT",
    };
    f.render_widget(keys(kb), root[4]);
}

// ─── Disc widget ──────────────────────────────────────────────────────────────

fn draw_disc(f: &mut Frame, state: &AppState, area: Rect) {
    let pumping = state.pump_frames > 0;
    let angle = state.disc_angle;
    let running = state.session == SessionState::Running;
    let speed_frac = (state.stats.current_speed_kmh / 6.0).clamp(0.0, 1.0) as f64;

    // ── Aspect-ratio correction ────────────────────────────────────────────
    // ratatui Canvas uses braille markers: 2 dots/char wide, 4 dots/char tall.
    // At standard 2:1 cell aspect (8×16 px), braille dots are square (4×4 px).
    // For a coordinate-space circle to render as a physical circle:
    //   x_range / (ew_chars * 2 dots) = y_range / (eh_chars * 4 dots)
    //   → y_range = x_range * (eh * 4) / (ew * 2) = x_range * 2 * eh / ew
    //
    // We fix x_range = 20 and compute y_range from the actual area.
    // For the fixed 22×12 disc panel: ew=20, eh=10 → y_range = 20*2*10/20 = 20
    // → both bounds [-10,10], making the coordinate space isometric.
    let ew = area.width.saturating_sub(2).max(1) as f64;
    let eh = area.height.saturating_sub(2).max(1) as f64;
    let x_ext = 10.0_f64;
    let y_ext = x_ext * 2.0 * eh / ew;

    // Disc radii in coordinate units (scale with y_ext so they fill the space)
    let scale = y_ext / 10.0; // 1.0 for the nominal 22×12 panel
    let r_outer: f64 = (if pumping { 8.5 } else { 8.0 }) * scale;
    let r_inner: f64 = (if pumping { 5.5 } else { 6.0 }) * scale;
    let r_hub: f64 = 1.8 * scale;

    let sweep_len = PI * 0.55;

    let ring_color = if pumping {
        C_PUMP
    } else {
        match state.session {
            SessionState::Running => C_CYAN,
            SessionState::Paused => C_AMBER,
            SessionState::Idle => C_DIM,
        }
    };

    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(C_DIM))
                .style(Style::default().bg(C_BG))
                .title(Span::styled(" DISC ", Style::default().fg(C_DIM))),
        )
        .x_bounds([-x_ext, x_ext])
        .y_bounds([-y_ext, y_ext])
        .paint(move |ctx| {
            draw_ring(ctx, 0.0, 0.0, r_outer, r_inner, C_DIM, 80);

            if running || pumping {
                draw_arc_ring(
                    ctx,
                    ArcRing {
                        cx: 0.0,
                        cy: 0.0,
                        r_out: r_outer,
                        r_in: r_inner,
                        a_start: angle,
                        a_end: angle + sweep_len,
                        color: C_SWEEP,
                        steps: 60,
                    },
                );
            }

            if running {
                let tx = (angle + sweep_len).cos() * ((r_outer + r_inner) / 2.0);
                let ty = (angle + sweep_len).sin() * ((r_outer + r_inner) / 2.0);
                ctx.draw(&Circle {
                    x: tx,
                    y: ty,
                    radius: 0.6 * scale,
                    color: Color::White,
                });
            }

            if running {
                let fill_end = speed_frac * TAU;
                draw_arc_ring(
                    ctx,
                    ArcRing {
                        cx: 0.0,
                        cy: 0.0,
                        r_out: r_inner - 0.3 * scale,
                        r_in: r_inner - 1.5 * scale,
                        a_start: -PI / 2.0,
                        a_end: -PI / 2.0 + fill_end,
                        color: lerp_color(C_BAR_LO, C_BAR_HI, speed_frac),
                        steps: 50,
                    },
                );
            }

            ctx.draw(&Circle {
                x: 0.0,
                y: 0.0,
                radius: r_hub,
                color: ring_color,
            });
        });

    f.render_widget(canvas, area);

    // Overlay the speed value centred in the disc area (hub text)
    let hub_label = if state.stats.current_speed_kmh > 0.0 {
        format!("{:.1}", state.stats.current_speed_kmh)
    } else {
        "·".into()
    };
    // Place it in the vertical centre of the canvas area
    let text_y = area.y + area.height / 2;
    let text_x = area.x + 1;
    let text_w = area.width.saturating_sub(2);
    if text_y < area.y + area.height {
        let overlay_rect = Rect {
            x: text_x,
            y: text_y,
            width: text_w,
            height: 1,
        };
        let p = Paragraph::new(Span::styled(
            hub_label,
            Style::default()
                .fg(if pumping { C_PUMP } else { C_PRIMARY })
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center)
        .style(Style::default().bg(C_BG));
        f.render_widget(p, overlay_rect);
    }
}

/// Draw a full ring (annulus) using small arc segments.
fn draw_ring(
    ctx: &mut Context,
    cx: f64,
    cy: f64,
    r_out: f64,
    r_in: f64,
    color: Color,
    steps: usize,
) {
    draw_arc_ring(
        ctx,
        ArcRing {
            cx,
            cy,
            r_out,
            r_in,
            a_start: 0.0,
            a_end: TAU,
            color,
            steps,
        },
    );
}

/// Draw an arc segment of a ring from `a_start` to `a_end` (radians).
struct ArcRing {
    cx: f64,
    cy: f64,
    r_out: f64,
    r_in: f64,
    a_start: f64,
    a_end: f64,
    color: Color,
    steps: usize,
}

fn draw_arc_ring(ctx: &mut Context, arc: ArcRing) {
    let da = (arc.a_end - arc.a_start) / arc.steps as f64;
    let r_mid = (arc.r_out + arc.r_in) / 2.0;
    let thickness = (arc.r_out - arc.r_in) / 2.0;
    for i in 0..arc.steps {
        let a = arc.a_start + i as f64 * da;
        let x = arc.cx + a.cos() * r_mid;
        let y = arc.cy + a.sin() * r_mid;
        ctx.draw(&Circle {
            x,
            y,
            radius: thickness.max(0.3),
            color: arc.color,
        });
    }
}

/// Linear interpolation between two `Color::Rgb` values by fraction `t` ∈ [0,1].
fn lerp_color(a: Color, b: Color, t: f64) -> Color {
    if let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) {
        let lerp = |lo: u8, hi: u8| (lo as f64 + (hi as f64 - lo as f64) * t) as u8;
        Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
    } else {
        a
    }
}

// ─── Stats grid ───────────────────────────────────────────────────────────────

fn draw_stats(f: &mut Frame, state: &AppState, area: Rect) {
    // 2 rows × 3 cells, each row is Length(4) = border+label+value+border.
    // The 12-row stats area has 4 rows of headroom (centred via Min padding).
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // top padding
            Constraint::Length(4), // row 1: TIME / SPEED / DISTANCE
            Constraint::Length(4), // row 2: STEPS / KCAL / STATE
            Constraint::Min(0),    // bottom padding
        ])
        .split(area);

    let stats = &state.stats;
    let sess_color = session_color(&state.session);

    let top_cells = [
        ("TIME", AppState::format_time(stats.active_seconds), C_CYAN),
        (
            "SPEED",
            format!("{:.1} km/h", stats.current_speed_kmh),
            sess_color,
        ),
        (
            "DISTANCE",
            format!("{:.2} km", stats.total_distance_km),
            C_PRIMARY,
        ),
    ];
    let bot_cells = [
        ("STEPS", format!("{}", stats.total_steps), C_PRIMARY),
        ("KCAL", format!("{:.0}", stats.calories), C_AMBER),
        ("STATE", state.status_label().to_string(), sess_color),
    ];

    render_stat_row(f, rows[1], &top_cells);
    render_stat_row(f, rows[2], &bot_cells);
}

fn render_stat_row(f: &mut Frame, area: Rect, cells: &[(&str, String, Color); 3]) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(area);

    for (i, (label, value, color)) in cells.iter().enumerate() {
        let p = Paragraph::new(vec![
            Line::from(Span::styled(*label, Style::default().fg(C_DIM))),
            Line::from(Span::styled(
                value.as_str(),
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(C_DIM))
                .style(Style::default().bg(C_BG)),
        )
        .alignment(Alignment::Center);
        f.render_widget(p, cols[i]);
    }
}

// ─── Clock widget (7-segment LCD style) ──────────────────────────────────────

fn draw_clock(f: &mut Frame, state: &AppState, area: Rect) {
    let now = chrono::Local::now();
    let time_str = now.format("%H:%M:%S").to_string();

    // Derive the session start time from the treadmill's elapsed-seconds counter
    // rather than `session_start_at` — the latter gets set on first observation of
    // the running belt and is wrong after a reconnect mid-session.
    let started_str = if state.stats.active_seconds > 0 {
        (now - chrono::Duration::seconds(state.stats.active_seconds as i64))
            .format("%H:%M:%S")
            .to_string()
    } else {
        "—".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(C_DIM))
        .style(Style::default().bg(C_BG))
        .title(Span::styled(" CLOCK ", Style::default().fg(C_DIM)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // top pad
            Constraint::Length(5), // big digits
            Constraint::Length(1), // spacer
            Constraint::Length(1), // STARTED line
            Constraint::Min(0),    // bottom pad
        ])
        .split(inner);

    let big = Paragraph::new(big_digit_lines(&time_str, C_PRIMARY)).alignment(Alignment::Center);
    f.render_widget(big, rows[1]);

    let started_line = Line::from(vec![
        Span::styled("STARTED ", Style::default().fg(C_DIM)),
        Span::styled(
            started_str,
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        ),
    ]);
    let sp = Paragraph::new(started_line).alignment(Alignment::Center);
    f.render_widget(sp, rows[3]);
}

/// Render a string (digits + ':') as 5 lines of chunky block-character glyphs.
fn big_digit_lines(s: &str, color: Color) -> Vec<Line<'static>> {
    let mut rows: [String; 5] = Default::default();
    for (i, ch) in s.chars().enumerate() {
        let glyph = digit_glyph(ch);
        for (r, row_buf) in rows.iter_mut().enumerate() {
            if i > 0 {
                row_buf.push(' ');
            }
            row_buf.push_str(glyph[r]);
        }
    }
    rows.into_iter()
        .map(|s| Line::from(Span::styled(s, Style::default().fg(color))))
        .collect()
}

/// 7-segment-ish glyph for a single time character. Each glyph is 4 cols × 5 rows.
fn digit_glyph(c: char) -> [&'static str; 5] {
    match c {
        '0' => [" ▄▄ ", "█  █", "█  █", "█  █", " ▀▀ "],
        '1' => ["  ▄ ", " ▀█ ", "  █ ", "  █ ", " ▀▀▀"],
        '2' => [" ▄▄ ", "▀  █", " ▄▄▀", "█   ", "▀▀▀▀"],
        '3' => ["▄▄▄ ", "   █", " ▀▀▄", "   █", "▀▀▀ "],
        '4' => ["█  █", "█  █", "▀▀▀█", "   █", "   ▀"],
        '5' => ["▄▄▄▄", "█   ", "▀▀▀▄", "   █", "▀▀▀ "],
        '6' => [" ▄▄▄", "█   ", "█▀▀▄", "█  █", " ▀▀ "],
        '7' => ["▀▀▀█", "   █", "  █ ", "  █ ", "  ▀ "],
        '8' => [" ▄▄ ", "█  █", " ▀▀ ", "█  █", " ▀▀ "],
        '9' => [" ▄▄ ", "█  █", " ▀▀█", "   █", " ▀▀ "],
        ':' => ["    ", "  ▄ ", "    ", "  ▀ ", "    "],
        _ => ["    ", "    ", "    ", "    ", "    "],
    }
}

// ─── Speed gauge ──────────────────────────────────────────────────────────────

fn draw_gauge(f: &mut Frame, state: &AppState, area: Rect) {
    let ratio = if state.max_speed_raw > 0 {
        (state.stats.current_speed_kmh / (state.max_speed_raw as f32 / 10.0)).clamp(0.0, 1.0) as f64
    } else {
        0.0
    };

    // Colour shifts from green → amber as speed approaches max
    let bar_color = lerp_color(C_BAR_LO, C_BAR_HI, ratio);

    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(C_DIM))
                .title(Span::styled(
                    format!(
                        " BELT  {:.1} / {:.1} km/h ",
                        state.stats.current_speed_kmh,
                        state.max_speed_raw as f32 / 10.0,
                    ),
                    Style::default().fg(C_PRIMARY),
                ))
                .style(Style::default().bg(C_BG)),
        )
        .gauge_style(Style::default().fg(bar_color).bg(C_BG))
        .label("")
        .ratio(ratio);
    f.render_widget(gauge, area);
}

// ─── Log panel ────────────────────────────────────────────────────────────────

fn draw_log(f: &mut Frame, state: &AppState, area: Rect) {
    let n = state.log_lines.len();
    let log_text: Vec<Line> = state
        .log_lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            // Most recent line brighter
            let color = if i + 1 == n { C_PRIMARY } else { C_DIM };
            Line::from(Span::styled(l.as_str(), Style::default().fg(color)))
        })
        .collect();

    let log = Paragraph::new(log_text)
        .block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                .border_style(Style::default().fg(C_DIM))
                .style(Style::default().bg(C_BG)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(log, area);
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

fn header_bar<'a>(prefix: &'a str, status: &'a str, status_color: Color) -> Paragraph<'a> {
    Paragraph::new(Line::from(vec![
        Span::styled(
            prefix,
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            status,
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(C_PRIMARY))
            .style(Style::default().bg(C_BG)),
    )
    .alignment(Alignment::Left)
}

fn keys(text: &str) -> Paragraph<'_> {
    Paragraph::new(Line::from(Span::styled(
        text,
        Style::default().fg(C_DIM).add_modifier(Modifier::ITALIC),
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(C_PRIMARY))
            .style(Style::default().bg(C_BG)),
    )
    .alignment(Alignment::Center)
}

fn session_color(state: &SessionState) -> Color {
    match state {
        SessionState::Running => C_PRIMARY,
        SessionState::Paused => C_AMBER,
        SessionState::Idle => C_DIM,
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DiscoveredDevice;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn session_state(session: SessionState, connected: bool) -> AppState {
        let mut s = AppState::new("WalkingPad".into());
        s.mode = AppMode::Session;
        s.session = session;
        s.ble_connected = connected;
        s
    }

    fn scanning_state() -> AppState {
        AppState::new("WalkingPad".into())
    }

    #[test]
    fn test_render_idle_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = session_state(SessionState::Idle, false);
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_running_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = session_state(SessionState::Running, true);
        state.stats.update(50, 100, 30);
        state.stats.tick_second();
        state.tick_animation();
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_running_pumping_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = session_state(SessionState::Running, true);
        state.stats.update(50, 100, 30);
        state.pump_frames = 3;
        state.disc_angle = std::f64::consts::PI;
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_speed_input_popup_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = session_state(SessionState::Running, true);
        state.speed_input.visible = true;
        state.speed_input.digits = "35".into();
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_speed_input_popup_empty_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = session_state(SessionState::Running, true);
        state.speed_input.visible = true;
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_paused_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = session_state(SessionState::Paused, true);
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_with_log_lines() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = session_state(SessionState::Running, true);
        state.push_log(">> connecting to WalkingPad");
        state.push_log(">> session started");
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_scanning_empty_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = scanning_state();
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_scanning_with_devices_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = scanning_state();
        state.add_scan_result(DiscoveredDevice {
            name: "KingSmith Z1".into(),
            addr: "AA:BB:CC:DD:EE:FF".into(),
            rssi: Some(-65),
        });
        state.add_scan_result(DiscoveredDevice {
            name: "Unknown".into(),
            addr: "11:22:33:44:55:66".into(),
            rssi: None,
        });
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_connecting_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("WalkingPad".into());
        state.mode = AppMode::Connecting;
        state.connecting_addr = Some("AA:BB:CC:DD:EE:FF".into());
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_config_dialog_no_fit_does_not_panic() {
        use crate::ui::app::ConfigDialog;
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("WalkingPad".into());
        state.config_dialog = ConfigDialog {
            visible: true,
            selected: 1,
            input_client_id: String::new(),
            input_client_secret: String::new(),
            device_display: "WalkingPad (AA:BB:CC:DD:EE:FF)".into(),
            fit_token_saved: false,
        };
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_config_dialog_with_fit_does_not_panic() {
        use crate::ui::app::ConfigDialog;
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("WalkingPad".into());
        state.config_dialog = ConfigDialog {
            visible: true,
            selected: 3,
            input_client_id: "my-client-id".into(),
            input_client_secret: "my-secret-key".into(),
            device_display: "WalkingPad (AA:BB:CC:DD:EE:FF)".into(),
            fit_token_saved: true,
        };
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_session_summary_does_not_panic() {
        use crate::ui::app::SessionSummary;
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("WalkingPad".into());
        state.mode = AppMode::SessionSummary;
        state.summary = Some(SessionSummary {
            duration_secs: 1334,
            steps: 2847,
            distance_km: 1.42,
            calories: 85.0,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            ended_at: std::time::SystemTime::UNIX_EPOCH,
        });
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_session_summary_with_fit_does_not_panic() {
        use crate::ui::app::SessionSummary;
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("WalkingPad".into());
        state.mode = AppMode::SessionSummary;
        state.fit_client_id = Some("client123".into());
        state.fit_client_secret = Some("secret456".into());
        state.summary = Some(SessionSummary {
            duration_secs: 1334,
            steps: 2847,
            distance_km: 1.42,
            calories: 85.0,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            ended_at: std::time::SystemTime::UNIX_EPOCH,
        });
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_connecting_no_addr_does_not_panic() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("WalkingPad".into());
        state.mode = AppMode::Connecting;
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_session_clock_wide_terminal() {
        // 140 cols leaves room for disc(44) + stats(>=30) + clock(44).
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = session_state(SessionState::Running, true);
        state.session_start_at = Some(std::time::SystemTime::UNIX_EPOCH);
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_render_session_clock_narrow_terminal() {
        // 80 cols clips the clock cell; should still render without panicking.
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = session_state(SessionState::Idle, false);
        terminal.draw(|f| draw(f, &state)).unwrap();
    }

    #[test]
    fn test_big_digit_lines_produces_five_rows() {
        let lines = big_digit_lines("12:34:56", C_PRIMARY);
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_format_time_zero() {
        assert_eq!(AppState::format_time(0), "0:00:00");
    }

    #[test]
    fn test_format_time_one_hour_35s() {
        assert_eq!(AppState::format_time(3635), "1:00:35");
    }

    #[test]
    fn test_format_time_95_secs() {
        assert_eq!(AppState::format_time(95), "0:01:35");
    }
}
