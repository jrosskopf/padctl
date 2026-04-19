use std::time::UNIX_EPOCH;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::error::{AppError, Result};
use crate::ui::app::SessionSummary;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const AUTH_URL: &str  = "https://accounts.google.com/o/oauth2/v2/auth";
const FIT_BASE: &str  = "https://www.googleapis.com/fitness/v1/users/me";

/// Ensure we have a valid access token.
///
/// If a refresh token is already saved, uses it to get a new access token silently.
/// Otherwise, runs the loopback OAuth 2.0 flow: opens the user's browser, binds a
/// local TCP listener to capture the redirect, exchanges the code for tokens, and
/// persists the refresh token to `padctl.toml`.
pub async fn ensure_token(cfg: &mut AppConfig) -> Result<String> {
    let client_id = cfg.fit_client_id.as_ref()
        .ok_or_else(|| AppError::Config("fit_client_id not configured".into()))?
        .clone();
    let client_secret = cfg.fit_client_secret.as_ref()
        .ok_or_else(|| AppError::Config("fit_client_secret not configured".into()))?
        .clone();

    if let Some(refresh_token) = cfg.fit_refresh_token.clone() {
        let client = reqwest::Client::new();
        let resp = client.post(TOKEN_URL)
            .form(&[
                ("client_id",     client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("grant_type",    "refresh_token"),
            ])
            .send().await
            .map_err(|e| AppError::Fit(e.to_string()))?;

        let body: Value = resp.json().await
            .map_err(|e| AppError::Fit(e.to_string()))?;

        return body["access_token"].as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| AppError::Fit(format!("Token refresh failed: {body}")));
    }

    // ── Loopback OAuth flow ──────────────────────────────────────────────
    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = TcpListener::bind("127.0.0.1:0").await
        .map_err(|e| AppError::Fit(format!("Cannot bind OAuth listener: {e}")))?;
    let port = listener.local_addr()
        .map_err(|e| AppError::Fit(e.to_string()))?.port();
    let redirect_uri = format!("http://localhost:{port}");

    let scope = "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Ffitness.activity.write";
    let redirect_enc = redirect_uri.replace(':', "%3A").replace('/', "%2F");
    let auth_url = format!(
        "{AUTH_URL}?client_id={client_id}&redirect_uri={redirect_enc}\
         &response_type=code&scope={scope}&access_type=offline&prompt=consent"
    );

    open::that(&auth_url)
        .map_err(|e| AppError::Fit(format!("Failed to open browser: {e}")))?;

    // Wait for the single redirect from the browser
    let (mut stream, _) = listener.accept().await
        .map_err(|e| AppError::Fit(format!("OAuth redirect listener failed: {e}")))?;

    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await
        .map_err(|e| AppError::Fit(e.to_string()))?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Reply so the browser doesn't hang
    let html_resp = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><body><h2>Authorization complete.</h2>\
        <p>You can close this tab and return to padctl.</p></body></html>";
    let _ = stream.write_all(html_resp.as_bytes()).await;

    // Parse "GET /?code=XXXX&... HTTP/1.1" from request line
    let code = request.lines()
        .next()
        .and_then(|line| line.split('?').nth(1))
        .and_then(|qs| qs.split(' ').next())
        .and_then(|qs| {
            qs.split('&')
                .find(|p| p.starts_with("code="))
                .map(|p| p[5..].to_string())
        })
        .ok_or_else(|| AppError::Fit("No authorization code in OAuth redirect".into()))?;

    // Exchange code → tokens
    let client = reqwest::Client::new();
    let resp = client.post(TOKEN_URL)
        .form(&[
            ("client_id",     client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code",          code.as_str()),
            ("redirect_uri",  redirect_uri.as_str()),
            ("grant_type",    "authorization_code"),
        ])
        .send().await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    let body: Value = resp.json().await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    let access_token = body["access_token"].as_str()
        .ok_or_else(|| AppError::Fit(format!("No access_token in token response: {body}")))?
        .to_string();

    // Persist refresh token so future runs don't need browser auth
    if let Some(rt) = body["refresh_token"].as_str() {
        cfg.fit_refresh_token = Some(rt.to_string());
        let _ = cfg.save();
    }

    Ok(access_token)
}

/// Upload a completed session to Google Fit.
///
/// Makes three sequential API calls:
/// 1. Ensure data sources exist (steps, distance, calories) — idempotent.
/// 2. Patch datasets with the session's data points.
/// 3. Create the treadmill session record.
pub async fn upload(token: &str, summary: &SessionSummary) -> Result<()> {
    let client = reqwest::Client::new();

    let start_ns = ts_nanos(&summary.started_at)?;
    let end_ns   = ts_nanos(&summary.ended_at)?;
    let start_ms = start_ns / 1_000_000;
    let end_ms   = end_ns   / 1_000_000;

    // ── Data sources ─────────────────────────────────────────────────────
    let steps_id = ensure_data_source(&client, token, "com.google.step_count.delta").await?;
    let dist_id  = ensure_data_source(&client, token, "com.google.distance.delta").await?;
    let cal_id   = ensure_data_source(&client, token, "com.google.calories.expended").await?;

    // ── Datasets ─────────────────────────────────────────────────────────
    patch_dataset(&client, token, &steps_id, start_ns, end_ns, json!({
        "minStartTimeNs": start_ns.to_string(),
        "maxEndTimeNs":   end_ns.to_string(),
        "dataSourceId":   steps_id,
        "point": [{
            "startTimeNanos": start_ns.to_string(),
            "endTimeNanos":   end_ns.to_string(),
            "dataTypeName":   "com.google.step_count.delta",
            "value": [{"intVal": summary.steps}]
        }]
    })).await?;

    patch_dataset(&client, token, &dist_id, start_ns, end_ns, json!({
        "minStartTimeNs": start_ns.to_string(),
        "maxEndTimeNs":   end_ns.to_string(),
        "dataSourceId":   dist_id,
        "point": [{
            "startTimeNanos": start_ns.to_string(),
            "endTimeNanos":   end_ns.to_string(),
            "dataTypeName":   "com.google.distance.delta",
            "value": [{"fpVal": (summary.distance_km * 1000.0) as f64}]
        }]
    })).await?;

    patch_dataset(&client, token, &cal_id, start_ns, end_ns, json!({
        "minStartTimeNs": start_ns.to_string(),
        "maxEndTimeNs":   end_ns.to_string(),
        "dataSourceId":   cal_id,
        "point": [{
            "startTimeNanos": start_ns.to_string(),
            "endTimeNanos":   end_ns.to_string(),
            "dataTypeName":   "com.google.calories.expended",
            "value": [{"fpVal": summary.calories as f64}]
        }]
    })).await?;

    // ── Session ───────────────────────────────────────────────────────────
    let session_id = format!("padctl-{start_ms}");
    let url = format!("{FIT_BASE}/sessions/{session_id}");
    let body = json!({
        "id":              session_id,
        "name":            "WalkingPad",
        "description":     "Kingfisher Z1 / KingSmith WalkingPad session",
        "activityType":    72,   // Treadmill (walking on a treadmill)
        "startTimeMillis": start_ms.to_string(),
        "endTimeMillis":   end_ms.to_string(),
        "application": {
            "packageName": "padctl",
            "name":        "padctl",
            "version":     "1"
        }
    });

    let resp = client.put(&url)
        .bearer_auth(token)
        .json(&body)
        .send().await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let rb: Value = resp.json().await
            .unwrap_or_else(|_| json!({"error": "unparseable"}));
        return Err(AppError::Fit(format!("Session create failed ({status}): {rb}")));
    }

    Ok(())
}

/// Spawn a background tokio task that calls `ensure_token` then `upload`.
/// Progress messages are forwarded via `log_tx` so the TUI can display them.
pub fn sync_in_background(mut cfg: AppConfig, summary: SessionSummary, log_tx: mpsc::Sender<String>) {
    tokio::spawn(async move {
        let _ = log_tx.send(">> Connecting to Google Fit…".into()).await;
        match ensure_token(&mut cfg).await {
            Ok(token) => {
                match upload(&token, &summary).await {
                    Ok(()) => {
                        let _ = log_tx.send(">> Google Fit: session synced ✓".into()).await;
                    }
                    Err(e) => {
                        let _ = log_tx.send(format!("!! Fit upload: {e}")).await;
                    }
                }
            }
            Err(e) => {
                let _ = log_tx.send(format!("!! Fit auth: {e}")).await;
            }
        }
    });
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn ts_nanos(t: &std::time::SystemTime) -> Result<i64> {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .map_err(|e| AppError::Fit(format!("Timestamp error: {e}")))
}

async fn ensure_data_source(
    client: &reqwest::Client,
    token: &str,
    data_type: &str,
) -> Result<String> {
    let url = format!("{FIT_BASE}/dataSources");
    let body = json!({
        "dataTypeName": data_type,
        "type": "raw",
        "application": {
            "packageName": "padctl",
            "name":        "padctl",
            "version":     "1"
        }
    });

    let resp = client.post(&url)
        .bearer_auth(token)
        .json(&body)
        .send().await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    let status = resp.status();
    let rb: Value = resp.json().await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    if status.is_success() {
        return parse_data_stream_id(&rb)
            .ok_or_else(|| AppError::Fit(format!("No dataStreamId in data source response: {rb}")));
    }

    if status.as_u16() == 409 {
        return fetch_existing_data_source(client, token, data_type).await;
    }

    Err(AppError::Fit(format!("Create data source failed ({status}): {rb}")))
}

async fn patch_dataset(
    client: &reqwest::Client,
    token: &str,
    source_id: &str,
    start_ns: i64,
    end_ns: i64,
    body: Value,
) -> Result<()> {
    let dataset_id = format!("{start_ns}-{end_ns}");
    let url = format!("{FIT_BASE}/dataSources/{source_id}/datasets/{dataset_id}");

    let resp = client.patch(&url)
        .bearer_auth(token)
        .json(&body)
        .send().await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let rb: Value = resp.json().await
            .unwrap_or_else(|_| json!({"error": "unparseable"}));
        return Err(AppError::Fit(format!("Dataset patch failed ({status}): {rb}")));
    }

    Ok(())
}

fn parse_data_stream_id(body: &Value) -> Option<String> {
    body["dataStreamId"].as_str().map(str::to_owned)
}

fn find_data_stream_id(body: &Value, data_type: &str) -> Option<String> {
    body.as_array().and_then(|sources| {
        sources.iter().find_map(|source| {
            let source_type = source["dataType"]["name"]
                .as_str()
                .or_else(|| source["dataTypeName"].as_str());
            match source_type {
                Some(name) if name == data_type => parse_data_stream_id(source),
                _ => None,
            }
        })
    })
}

async fn fetch_existing_data_source(
    client: &reqwest::Client,
    token: &str,
    data_type: &str,
) -> Result<String> {
    let url = format!("{FIT_BASE}/dataSources");
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Fit(e.to_string()))?;

    if !status.is_success() {
        return Err(AppError::Fit(format!(
            "List data sources failed ({status}): {body}"
        )));
    }

    find_data_stream_id(&body, data_type).ok_or_else(|| {
        AppError::Fit(format!(
            "Existing data source for {data_type} not found after conflict: {body}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{find_data_stream_id, parse_data_stream_id};
    use serde_json::json;

    #[test]
    fn test_parse_data_stream_id_present() {
        let body = json!({ "dataStreamId": "derived:com.google.step_count.delta" });
        assert_eq!(
            parse_data_stream_id(&body).as_deref(),
            Some("derived:com.google.step_count.delta")
        );
    }

    #[test]
    fn test_parse_data_stream_id_missing() {
        let body = json!({ "error": { "code": 409 } });
        assert_eq!(parse_data_stream_id(&body), None);
    }

    #[test]
    fn test_find_data_stream_id_from_list() {
        let body = json!([
            {
                "dataType": { "name": "com.google.step_count.delta" },
                "dataStreamId": "raw:steps"
            },
            {
                "dataTypeName": "com.google.distance.delta",
                "dataStreamId": "raw:distance"
            }
        ]);

        assert_eq!(
            find_data_stream_id(&body, "com.google.distance.delta").as_deref(),
            Some("raw:distance")
        );
    }
}
