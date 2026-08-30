//! Anonymous usage telemetry, emitting the DataZoo `telemetry_schema: 2`
//! envelope so this product is comparable with the rest of the stack.
//!
//! **This file is shared verbatim between `cicdbar` and `padctl`.** It carries
//! no product-specific vocabulary -- the `feature` values are registered by the
//! host via `with_features` -- so the two copies must stay byte-identical.
//! Each repo runs the same envelope test; if the copies drift, those fail.
//!
//! The canonical implementation is the C++ library
//! `DataZooDE/posthog-telemetry`. This is a Rust port of the same contract --
//! linking the C++ one would drag a C++ toolchain into the static musl build.
//! `docs/TELEMETRY-SCHEMA.md` is vendored beside it, and the envelope test in
//! `tests/telemetry.rs` fails if the two drift.
//!
//! Three rules govern everything here:
//!
//! 1. **Enumerated values only.** A property value must come from a small set
//!    the code controls. Never a repo name, a branch, a path, a token, or an
//!    error message. `Value::from` a caller-supplied string is *not* enough --
//!    the emit path filters to a known allow-list.
//! 2. **Never any money.** cicdbar can see what CI costs someone; telemetry
//!    cannot. Not amounts, not budgets, not buckets of either.
//! 3. **Never harm the host.** Delivery is best-effort with a hard timeout;
//!    telemetry failing must be indistinguishable from telemetry succeeding.

use std::time::Duration;

/// PostHog *project* key. `phc_` keys are client-side by design and are not a
/// secret; this is the same project the rest of the DataZoo stack reports to.
const API_KEY: &str = "phc_t3wwRLtpyEmLHYaZCSszG0MqVr74J6wnCrj9D41zk2t";
const DEFAULT_HOST: &str = "https://eu.i.posthog.com";
const SCHEMA: u8 = 2;
const CLAMP: usize = 512;
/// Salt so a `distinct_id` cannot be reproduced from a raw machine id held in
/// some other dataset.
const SALT: &str = "datazoo-telemetry-v2";

/// Property values telemetry is allowed to carry.
#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Str(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Str(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::Int(v)
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Value {
        Value::Int(v as i64)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Float(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

impl Value {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Str(s) => {
                let mut s = s.clone();
                if s.len() > CLAMP {
                    s.truncate(CLAMP);
                }
                serde_json::Value::String(s)
            }
            Value::Int(i) => serde_json::json!(i),
            Value::Float(f) => serde_json::json!(f),
            Value::Bool(b) => serde_json::json!(b),
        }
    }
}

/// Property names this product may ever send, and for enumerated string
/// properties, the exact values allowed.
///
/// This allow-list -- not the caller -- is what makes the privacy promise
/// true. Anything not named here is dropped before it can be serialised.
const ALLOWED_ENUM_PROPS: &[(&str, &[&str])] = &[
    (
        "feature",
        &[
            "spend_shown",
            "tooltip_rendered",
            "notification_sent",
            "blacksmith_dashboard",
            "blacksmith_estimate",
            "demo",
        ],
    ),
    (
        "error_class",
        &[
            "auth",
            "access_denied",
            "rate_limited",
            "unreachable",
            "decode",
            "other",
        ],
    ),
    (
        "phase",
        &["billing", "runs", "blacksmith", "notify", "render"],
    ),
    ("install_kind", &["waybar", "cli"]),
    // Bucket labels are strings, so they need enumerating like any other
    // string property -- otherwise the allow-list silently swallows the one
    // event this product exists to send.
    ("orgs_bucket", BUCKETS),
    ("repos_bucket", BUCKETS),
    ("ticks_bucket", BUCKETS),
];

/// The only bucket labels that may ever be sent. Exact counts identify an
/// install; these do not.
const BUCKETS: &[&str] = &["0", "1", "2-5", "6-20", "20+"];
/// Numeric and boolean properties that may be sent. Values are bucketed by the
/// caller; nothing here is money.
const ALLOWED_SCALAR_PROPS: &[&str] = &[
    "duration_ms",
    "blacksmith_enabled",
    "notifications_enabled",
    "dashboard_session_valid",
    "degraded",
    "call_count",
];

fn property_allowed(key: &str, value: &Value, features: &[&'static str]) -> bool {
    if let Value::Str(s) = value {
        if key == "feature" {
            // Registered by the host product, so this module stays free of
            // product-specific vocabulary and can be shared verbatim.
            return features.contains(&s.as_str());
        }
        return ALLOWED_ENUM_PROPS
            .iter()
            .any(|(k, allowed)| *k == key && allowed.contains(&s.as_str()));
    }
    ALLOWED_SCALAR_PROPS.contains(&key)
}

fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key).map(|v| truthy(&v)).unwrap_or(false)
}

/// The product-local kill switch, e.g. `cicdbar` -> `CICDBAR_NO_TELEMETRY`.
pub fn product_env_var(product: &str) -> String {
    format!(
        "{}_NO_TELEMETRY",
        product.to_ascii_uppercase().replace('-', "_")
    )
}

/// Any one of these disables telemetry entirely, enforced before anything
/// touches the network.
pub fn disabled_by_environment(product: &str) -> bool {
    env_truthy("DATAZOO_DISABLE_TELEMETRY")
        || env_truthy("DO_NOT_TRACK")
        || env_truthy(&product_env_var(product))
}

fn is_ci() -> bool {
    [
        "CI",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "BUILDKITE",
        "JENKINS_URL",
        "TEAMCITY_VERSION",
        "TF_BUILD",
        "CIRCLECI",
    ]
    .iter()
    .any(|k| std::env::var(k).is_ok())
}

fn is_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|c| c.contains("docker") || c.contains("containerd") || c.contains("kubepods"))
            .unwrap_or(false)
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())
}

/// How `distinct_id` was derived. Quality matters for retention analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    MachineId,
    Mac,
    Ephemeral,
}

impl IdentitySource {
    fn as_str(&self) -> &'static str {
        match self {
            IdentitySource::MachineId => "machine_id",
            IdentitySource::Mac => "mac",
            IdentitySource::Ephemeral => "ephemeral",
        }
    }
}

fn machine_identity() -> (String, IdentitySource) {
    for p in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(id) = std::fs::read_to_string(p) {
            let id = id.trim();
            if !id.is_empty() {
                return (
                    sha256_hex(&format!("{SALT}:{id}")),
                    IdentitySource::MachineId,
                );
            }
        }
    }
    if let Some(mac) = first_mac() {
        return (sha256_hex(&format!("{SALT}:{mac}")), IdentitySource::Mac);
    }
    // No stable hardware identity: a per-process id, explicitly marked so it
    // never creates or merges a Person.
    let seed = format!(
        "{SALT}:{}:{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    );
    (sha256_hex(&seed), IdentitySource::Ephemeral)
}

fn first_mac() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/net").ok()?;
    let mut macs: Vec<String> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != "lo")
        .filter_map(|e| std::fs::read_to_string(e.path().join("address")).ok())
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m != "00:00:00:00:00:00")
        .collect();
    macs.sort();
    macs.into_iter().next()
}

pub struct Telemetry {
    enabled: bool,
    product: String,
    version: String,
    host: String,
    distinct_id: String,
    identity: IdentitySource,
    session_id: String,
    features: Vec<&'static str>,
    queue: std::sync::Mutex<Vec<serde_json::Value>>,
}

impl Telemetry {
    /// Build for real use.
    pub fn new(product: &str, version: &str, force_off: bool) -> Telemetry {
        let enabled = !force_off && !disabled_by_environment(product);
        let (distinct_id, identity) = if enabled {
            machine_identity()
        } else {
            (String::new(), IdentitySource::Ephemeral)
        };
        Telemetry {
            enabled,
            product: product.to_string(),
            version: version.to_string(),
            host: std::env::var("DATAZOO_TELEMETRY_HOST").unwrap_or_else(|_| DEFAULT_HOST.into()),
            distinct_id,
            identity,
            session_id: sha256_hex(&format!(
                "{:?}{}",
                std::time::SystemTime::now(),
                std::process::id()
            ))[..32]
                .to_string(),
            features: Vec::new(),
            queue: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Register the `feature` values this product may emit. Anything not
    /// listed here is dropped, so the vocabulary stays small and enumerated
    /// -- and this module stays free of product-specific words, which is what
    /// lets the same file live in every Rust product unchanged.
    pub fn with_features(mut self, features: &[&'static str]) -> Telemetry {
        self.features = features.to_vec();
        self
    }

    /// Not used by every product; kept so the shared module stays identical.
    #[allow(dead_code)]
    pub fn disabled() -> Telemetry {
        Telemetry {
            enabled: false,
            product: String::new(),
            version: String::new(),
            host: String::new(),
            distinct_id: String::new(),
            identity: IdentitySource::Ephemeral,
            session_id: String::new(),
            features: Vec::new(),
            queue: std::sync::Mutex::new(Vec::new()),
        }
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn for_test(product: &str, version: &str, host: &str) -> Telemetry {
        let mut t = Telemetry::new(product, version, false);
        t.host = host.to_string();
        t
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn for_test_ephemeral(product: &str, version: &str, host: &str) -> Telemetry {
        let mut t = Telemetry::for_test(product, version, host);
        t.identity = IdentitySource::Ephemeral;
        t
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn envelope(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut p = serde_json::Map::new();
        let os = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux"
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "amd64"
        };
        p.insert("product".into(), self.product.clone().into());
        p.insert("product_version".into(), self.version.clone().into());
        p.insert("product_edition".into(), "oss".into());
        p.insert("telemetry_schema".into(), serde_json::json!(SCHEMA));
        p.insert("os".into(), os.into());
        p.insert("arch".into(), arch.into());
        p.insert("platform".into(), format!("{os}_{arch}").into());
        p.insert("is_ci".into(), serde_json::json!(is_ci()));
        p.insert("is_container".into(), serde_json::json!(is_container()));
        p.insert("$session_id".into(), self.session_id.clone().into());
        p.insert("identity_source".into(), self.identity.as_str().into());
        if self.identity == IdentitySource::Ephemeral || is_ci() {
            p.insert("$process_person_profile".into(), serde_json::json!(false));
        }
        p
    }

    /// Queue an event. Properties not on the allow-list are dropped, silently
    /// and by design -- the allow-list is the privacy guarantee, so a caller
    /// mistake must not become a leak.
    pub fn capture(&self, event: &str, props: Vec<(&str, Value)>) {
        if !self.enabled {
            return;
        }
        let mut p = self.envelope();
        for (k, v) in props {
            if property_allowed(k, &v, &self.features) {
                p.insert(k.to_string(), v.to_json());
            }
        }
        let ev = serde_json::json!({
            "event": event,
            "distinct_id": self.distinct_id,
            "properties": serde_json::Value::Object(p),
        });
        if let Ok(mut q) = self.queue.lock() {
            q.push(ev);
        }
    }

    /// Best-effort delivery. Never returns an error and never blocks the host
    /// program for long: a short timeout, and failures are simply dropped.
    pub fn flush(&self) {
        if !self.enabled {
            return;
        }
        let batch: Vec<serde_json::Value> = match self.queue.lock() {
            Ok(mut q) if !q.is_empty() => q.drain(..).collect(),
            _ => return,
        };
        let body = serde_json::json!({ "api_key": API_KEY, "batch": batch });
        let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(2000))
            .build()
        else {
            return;
        };
        let _ = client
            .post(format!("{}/batch/", self.host.trim_end_matches('/')))
            .json(&body)
            .send();
    }
}
