use reqwest::Client;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

const SERVER_BIN: &str = "l337-audio-server";

/// Find an available TCP port by binding to port 0 and reading the assigned port.
async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind ephemeral port");
    let addr = listener.local_addr().expect("Failed to get local addr");
    drop(listener);
    addr.port()
}

/// Spawn the server on an available port in dummy HTTP mode.
///
/// Uses `cargo run --bin l337-audio-server` so the binary does not need to be
/// pre-built. Returns the port and a handle to the child process.
async fn spawn_server() -> (u16, tokio::process::Child) {
    // Clean up any stale instance lock from previous test runs.
    let _ = std::fs::remove_file(
        std::env::temp_dir()
            .join("l337-audio-server-runtime")
            .join("instance.lock"),
    );
    let _ = std::fs::remove_file(
        dirs::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir())
            .join("l337")
            .join("l337-audio-server")
            .join("instance.lock"),
    );

    let port = find_available_port().await;

    let mut child = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg(SERVER_BIN)
        .arg("--")
        .arg("--dummy")
        .arg("--transport=http")
        .env("L337__SERVER__PORT", port.to_string())
        .env("L337__SERVER__HOST", "127.0.0.1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn l337-audio-server via cargo run");

    // Wait for the server to become ready by polling /health.
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let url = format!("https://127.0.0.1:{}/health", port);
    for _ in 0..120 {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return (port, child);
            }
        }
        sleep(Duration::from_millis(500)).await;
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
    panic!("Server did not become ready on port {}", port);
}

#[tokio::test]
async fn test_health_endpoint() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let resp = client
        .get(format!("https://127.0.0.1:{}/health", _port))
        .send()
        .await
        .expect("Failed to call /health");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("Failed to parse /health JSON");
    assert_eq!(body["status"], "ok");
    assert!(body["capabilities"].is_object());

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_setup_endpoint_returns_token() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("Failed to parse /setup JSON");
    assert!(body["token"].is_string());
    assert!(!body["token"].as_str().unwrap().is_empty());

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_root_endpoint() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let resp = client
        .get(format!("https://127.0.0.1:{}/", _port))
        .send()
        .await
        .expect("Failed to call /");

    assert_eq!(resp.status(), 200);
    let text = resp.text().await.expect("Failed to read / body");
    assert_eq!(text, "L337 Audio Server");

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_player_status_requires_auth() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .send()
        .await
        .expect("Failed to call /player/status");

    assert_eq!(resp.status(), 401);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_player_status_with_auth() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    // Discover token via /setup.
    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status with auth");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("Failed to parse /player/status JSON");
    assert_eq!(body["state"], "stopped");

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_play_endpoint() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let payload = serde_json::json!({
        "track_id": "test-track-1",
        "stream_url": "http://example.com/stream",
        "title": "Test Track",
        "artist": "Test Artist",
        "duration": 120
    });

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/play", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&payload)
        .send()
        .await
        .expect("Failed to call /player/play");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("Failed to parse /player/play JSON");
    assert_eq!(body["ok"], true);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_play_endpoint_validation() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let payload = serde_json::json!({
        "track_id": "",
        "stream_url": ""
    });

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/play", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&payload)
        .send()
        .await
        .expect("Failed to call /player/play");

    assert_eq!(resp.status(), 400);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_pause_endpoint() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/pause", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/pause");

    assert_eq!(resp.status(), 200);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_next_endpoint() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/next", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/next");

    assert_eq!(resp.status(), 200);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_previous_endpoint() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/previous", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/previous");

    assert_eq!(resp.status(), 200);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_player_settings_update() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let payload = serde_json::json!({
        "max_disk_pool_bytes": 512 * 1024 * 1024
    });

    let resp = client
        .put(format!("https://127.0.0.1:{}/player/settings", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&payload)
        .send()
        .await
        .expect("Failed to call /player/settings");

    assert_eq!(resp.status(), 200);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_cache_lookup_endpoint() {
    let (_port, mut child) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let setup_resp = client
        .get(format!("https://127.0.0.1:{}/setup", _port))
        .send()
        .await
        .expect("Failed to call /setup");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("Failed to parse /setup JSON");
    let token = setup_body["token"].as_str().unwrap();

    let payload = serde_json::json!({
        "track_ids": ["abc123", "def456"]
    });

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/lookup", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&payload)
        .send()
        .await
        .expect("Failed to call /player/cache/lookup");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("Failed to parse /player/cache/lookup JSON");
    assert!(body["cached"].is_array());

    let _ = child.kill().await;
    let _ = child.wait().await;
}
