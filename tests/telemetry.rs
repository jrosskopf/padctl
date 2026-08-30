//! Telemetry tests. No mocks: a real HTTP server on loopback stands in for
//! PostHog, and assertions are made on the JSON that actually arrived.
//!
//! This file mirrors the equivalent suite in cicdbar. The telemetry module is
//! shared byte-for-byte between the two products, so the envelope assertions
//! here are what catch the copies drifting apart.

use std::sync::mpsc;

// The module is part of the binary crate, so it is included directly.
#[path = "../src/telemetry.rs"]
mod telemetry;
use telemetry::{Telemetry, Value};

/// The opt-out tests set process-global environment variables, which every
/// other test in this binary can see. Cargo runs tests in parallel by
/// default, so without a guard an opt-out test can disable telemetry
/// underneath a concurrent test and make it fail -- which is exactly what
/// happened on macOS CI while Linux passed by luck of timing.
///
/// Every test here takes this lock, so the suite is correct under a plain
/// `cargo test` rather than relying on `--test-threads=1`.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn capture_server() -> (String, mpsc::Receiver<String>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
            let _ = tx.send(body);
            let _ = req.respond(tiny_http::Response::from_string("{\"status\":1}"));
        }
    });
    (url, rx)
}

fn recv(rx: &mpsc::Receiver<String>) -> serde_json::Value {
    let raw = rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("no telemetry arrived");
    serde_json::from_str(&raw).expect("payload must be json")
}

const FEATURES: &[&str] = &[
    "session_started",
    "session_ended",
    "device_connected",
    "google_fit_synced",
];

fn telemetry(url: &str) -> Telemetry {
    Telemetry::for_test("padctl", "9.9.9", url).with_features(FEATURES)
}

#[test]
fn every_event_carries_the_schema_2_envelope() {
    let _guard = serial();
    let (url, rx) = capture_server();
    let t = telemetry(&url);
    t.capture("cli_started", vec![]);
    t.flush();

    let body = recv(&rx);
    assert_eq!(&body["api_key"].as_str().unwrap()[..4], "phc_");
    let p = &body["batch"][0]["properties"];
    assert_eq!(p["product"], "padctl");
    assert_eq!(p["product_version"], "9.9.9");
    assert_eq!(p["product_edition"], "oss");
    assert_eq!(p["telemetry_schema"], 2);
    assert!(p["telemetry_schema"].is_number());
    assert!(["linux", "macos", "windows"].contains(&p["os"].as_str().unwrap()));
    assert!(["amd64", "arm64"].contains(&p["arch"].as_str().unwrap()));
    assert!(p["platform"].is_string());
    assert!(p["is_ci"].is_boolean());
    assert!(p["is_container"].is_boolean());
    assert!(p["$session_id"].is_string());
    assert!(["machine_id", "mac", "ephemeral"].contains(&p["identity_source"].as_str().unwrap()));
}

#[test]
fn the_product_is_padctl_and_the_env_var_follows_from_it() {
    let _guard = serial();
    assert_eq!(telemetry::product_env_var("padctl"), "PADCTL_NO_TELEMETRY");
    assert_eq!(
        telemetry::product_env_var("cicdbar"),
        "CICDBAR_NO_TELEMETRY"
    );
}

fn assert_silent(env_key: &str, env_val: &str) {
    let (url, rx) = capture_server();
    std::env::set_var(env_key, env_val);
    let t = Telemetry::for_test("padctl", "9.9.9", &url).with_features(FEATURES);
    t.capture("cli_started", vec![]);
    t.flush();
    std::env::remove_var(env_key);
    assert!(
        !t.is_enabled(),
        "{env_key}={env_val} must disable telemetry"
    );
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(400))
            .is_err(),
        "{env_key}={env_val} still sent something"
    );
}

#[test]
fn all_four_opt_outs_are_honoured() {
    let _guard = serial();
    assert_silent("DATAZOO_DISABLE_TELEMETRY", "1");
    assert_silent("DO_NOT_TRACK", "1");
    assert_silent("PADCTL_NO_TELEMETRY", "1");
    assert_silent("DATAZOO_DISABLE_TELEMETRY", "true");
}

#[test]
fn nothing_personal_can_reach_a_payload() {
    let _guard = serial();
    // padctl knows a device name, a MAC address, and Google Fit tokens.
    let secrets = [
        "KS-HD-Z1D",
        "AA:BB:CC:DD:EE:FF",
        "ya29.a0AfH6SMB-google-oauth-token",
        "jr@data-zoo.de",
        "/home/jr/padctl.toml",
    ];
    let (url, rx) = capture_server();
    let t = telemetry(&url);
    for s in secrets {
        t.capture("feature_used", vec![("feature", Value::from(s))]);
    }
    t.flush();
    let mut seen = String::new();
    while let Ok(b) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
        seen.push_str(&b);
    }
    for s in secrets {
        assert!(!seen.contains(s), "{s:?} reached a telemetry payload");
    }
}

#[test]
fn a_registered_feature_does_get_through() {
    let _guard = serial();
    let (url, rx) = capture_server();
    let t = telemetry(&url);
    t.capture(
        "feature_used",
        vec![
            ("feature", Value::from("session_ended")),
            ("duration_ms", Value::from(125_000i64)),
        ],
    );
    t.flush();
    let p = recv(&rx)["batch"][0]["properties"].clone();
    assert_eq!(p["feature"], "session_ended");
    assert_eq!(p["duration_ms"], 125_000);
}

#[test]
fn an_unreachable_endpoint_neither_fails_nor_hangs() {
    let _guard = serial();
    let t = Telemetry::for_test("padctl", "9.9.9", "http://127.0.0.1:1").with_features(FEATURES);
    let start = std::time::Instant::now();
    t.capture("cli_started", vec![]);
    t.flush();
    // The client's own timeout is 2s; this proves it does not hang, with
    // enough slack to survive a loaded CI runner.
    assert!(
        start.elapsed().as_secs_f64() < 8.0,
        "blocked for {:?}",
        start.elapsed()
    );
}
