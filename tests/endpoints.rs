use reqwest::Client;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

fn write_minimal_wav(path: &PathBuf) {
    let sample_rate = 44100u32;
    let channels = 2u16;
    let duration_secs = 1u32;
    let num_samples = (sample_rate * duration_secs) as usize;
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    let data_bytes = (num_samples * channels as usize) as u32 * 2;
    let chunk_size = 36 + data_bytes;

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&chunk_size.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for _ in 0..(num_samples * channels as usize) {
        bytes.extend_from_slice(&[0u8; 2]);
    }
    std::fs::write(path, bytes).unwrap();
}

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
/// pre-built. Returns the port, a handle to the child process, and the XDG
/// config directory used for the challenge token.
async fn spawn_server() -> (u16, tokio::process::Child, PathBuf) {
    let config_dir = std::env::temp_dir().join("l337-test-config").join("l337-audio-server");
    let _ = std::fs::create_dir_all(&config_dir);

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
    let _ = std::fs::remove_file(config_dir.join("instance.lock"));

    let port = find_available_port().await;

    let binary_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .and_then(|mut p| {
            p.pop();
            p.pop();
            p.push("bin");
            p.push("l337-audio-server");
            if p.exists() { Some(p) } else { None }
        })
        .or_else(|| {
            let mut p = std::env::current_dir().unwrap_or_default();
            p.push("bin");
            p.push("l337-audio-server");
            if p.exists() { Some(p) } else { None }
        })
        .unwrap_or_else(|| PathBuf::from("l337-audio-server"));

    let mut child = Command::new(binary_path)
        .arg("--dummy")
        .arg("--transport=http")
        .env("L337__SERVER__PORT", port.to_string())
        .env("L337__SERVER__HOST", "127.0.0.1")
        .env("XDG_CONFIG_HOME", config_dir.parent().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn l337-audio-server");

    // Wait for the server to become ready by polling /health.
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let url = format!("https://127.0.0.1:{}/health", port);
    for _ in 0..120 {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return (port, child, config_dir);
            }
        }
        sleep(Duration::from_millis(500)).await;
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
    panic!("Server did not become ready on port {}", port);
}

/// Obtain a valid bearer token by completing the challenge/redeem flow.
async fn get_token_via_challenge(client: &Client, port: u16, config_dir: &PathBuf) -> String {
    let challenge_resp = client
        .post(format!("https://127.0.0.1:{}/auth/challenge", port))
        .send()
        .await
        .expect("Failed to call /auth/challenge");
    assert_eq!(challenge_resp.status(), 202);

    let challenge_path = config_dir.join("challenge-token.txt");
    let challenge_token = std::fs::read_to_string(&challenge_path)
        .expect("Failed to read challenge-token.txt")
        .trim()
        .to_string();

    let redeem_resp = client
        .post(format!("https://127.0.0.1:{}/auth/redeem", port))
        .header("X-L337-Challenge", &challenge_token)
        .send()
        .await
        .expect("Failed to call /auth/redeem");
    assert_eq!(redeem_resp.status(), 200);

    challenge_token
}

#[tokio::test]
async fn test_health_endpoint() {
    let (_port, mut child, _config_dir) = spawn_server().await;
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
async fn test_root_endpoint() {
    let (_port, mut child, _config_dir) = spawn_server().await;
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
    let (_port, mut child, _config_dir) = spawn_server().await;
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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    // Discover token via /setup.
    let token = get_token_via_challenge(&client, _port, &config_dir).await;

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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let audio_path = std::env::temp_dir().join("l337-test-audio.wav");
    write_minimal_wav(&audio_path);

    let payload = serde_json::json!({
        "track_id": "test-track-1",
        "stream_url": format!("file://{}", audio_path.display()),
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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

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
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

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

#[tokio::test]
async fn test_seek_endpoint() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let audio_path = std::env::temp_dir().join("l337-test-seek.wav");
    write_minimal_wav(&audio_path);

    let play_payload = serde_json::json!({
        "track_id": "seek-test-track",
        "stream_url": format!("file://{}", audio_path.display()),
        "title": "Seek Test",
        "artist": "Test",
        "duration": 1
    });

    let play_resp = client
        .post(format!("https://127.0.0.1:{}/player/play", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&play_payload)
        .send()
        .await
        .expect("Failed to call /player/play");
    assert_eq!(play_resp.status(), 200);

    sleep(Duration::from_millis(200)).await;

    let seek_payload = serde_json::json!({
        "position": 0
    });

    let seek_resp = client
        .post(format!("https://127.0.0.1:{}/player/seek", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&seek_payload)
        .send()
        .await
        .expect("Failed to call /player/seek");
    assert_eq!(seek_resp.status(), 200);

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(status_resp.status(), 200);
    let status_body: serde_json::Value = status_resp.json().await.expect("Failed to parse /player/status JSON");
    assert_eq!(status_body["state"], "playing");
    assert!(status_body["position_sec"].is_number());
    assert!(status_body["duration_sec"].is_number());

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_upload_stream_endpoint() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let audio_path = std::env::temp_dir().join("l337-test-stream.wav");
    write_minimal_wav(&audio_path);
    let wav_bytes = std::fs::read(&audio_path).expect("Failed to read WAV bytes");

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/play/stream", _port))
        .header("Authorization", format!("Bearer {}", token))
        .header("X-Track-Id", "stream-test-track")
        .header("X-Title", "Stream Test")
        .header("X-Artist", "Test Artist")
        .body(wav_bytes)
        .send()
        .await
        .expect("Failed to call /player/play/stream");

    assert_eq!(resp.status(), 200);

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(status_resp.status(), 200);
    let status_body: serde_json::Value = status_resp.json().await.expect("Failed to parse /player/status JSON");
    assert_eq!(status_body["state"], "playing");
    assert_eq!(status_body["current_track"]["track_id"], "stream-test-track");
    assert_eq!(status_body["current_track"]["title"], "Stream Test");
    assert_eq!(status_body["audio_available"], true);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_upload_stream_validation() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/play/stream", _port))
        .header("Authorization", format!("Bearer {}", token))
        .body(vec![])
        .send()
        .await
        .expect("Failed to call /player/play/stream");

    assert_eq!(resp.status(), 400);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_auth_invalid_token() {
    let (_port, mut child, _config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", "Bearer wrong-token")
        .send()
        .await
        .expect("Failed to call /player/status");

    assert_eq!(resp.status(), 401);

    let resp2 = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", "NotBearer at-all")
        .send()
        .await
        .expect("Failed to call /player/status with malformed header");

    assert_eq!(resp2.status(), 401);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_speed_endpoint() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let audio_path = std::env::temp_dir().join("l337-test-speed.wav");
    write_minimal_wav(&audio_path);

    let play_payload = serde_json::json!({
        "track_id": "speed-test-track",
        "stream_url": format!("file://{}", audio_path.display()),
        "title": "Speed Test",
        "artist": "Test",
        "duration": 1
    });

    let play_resp = client
        .post(format!("https://127.0.0.1:{}/player/play", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&play_payload)
        .send()
        .await
        .expect("Failed to call /player/play");
    assert_eq!(play_resp.status(), 200);

    let speed_payload = serde_json::json!({"speed": 1.5});
    let speed_resp = client
        .post(format!("https://127.0.0.1:{}/player/speed", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&speed_payload)
        .send()
        .await
        .expect("Failed to call /player/speed");
    assert_eq!(speed_resp.status(), 200);

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(status_resp.status(), 200);
    let status_body: serde_json::Value = status_resp.json().await.expect("Failed to parse /player/status JSON");
    assert_eq!(status_body["speed"], 1.5);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_volume_endpoint() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let audio_path = std::env::temp_dir().join("l337-test-volume.wav");
    write_minimal_wav(&audio_path);

    let play_payload = serde_json::json!({
        "track_id": "volume-test-track",
        "stream_url": format!("file://{}", audio_path.display()),
        "title": "Volume Test",
        "artist": "Test",
        "duration": 1
    });

    let play_resp = client
        .post(format!("https://127.0.0.1:{}/player/play", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&play_payload)
        .send()
        .await
        .expect("Failed to call /player/play");
    assert_eq!(play_resp.status(), 200);

    let volume_payload = serde_json::json!({"volume": 0.5});
    let volume_resp = client
        .post(format!("https://127.0.0.1:{}/player/volume", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&volume_payload)
        .send()
        .await
        .expect("Failed to call /player/volume");
    assert_eq!(volume_resp.status(), 200);

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(status_resp.status(), 200);
    let status_body: serde_json::Value = status_resp.json().await.expect("Failed to parse /player/status JSON");
    assert_eq!(status_body["volume"], 0.5);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_cache_next_and_previous_endpoints() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let next_audio = std::env::temp_dir().join("l337-test-next.wav");
    write_minimal_wav(&next_audio);
    let prev_audio = std::env::temp_dir().join("l337-test-prev.wav");
    write_minimal_wav(&prev_audio);

    let cache_next_payload = serde_json::json!({
        "track_id": "next-cache-track",
        "stream_url": format!("file://{}", next_audio.display()),
        "title": "Next Track",
        "artist": "Test",
        "duration": 1
    });

    let cache_next_resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/next", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&cache_next_payload)
        .send()
        .await
        .expect("Failed to call /player/cache/next");
    assert_eq!(cache_next_resp.status(), 202);

    let cache_prev_payload = serde_json::json!({
        "track_id": "prev-cache-track",
        "stream_url": format!("file://{}", prev_audio.display()),
        "title": "Prev Track",
        "artist": "Test",
        "duration": 1
    });

    let cache_prev_resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/previous", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&cache_prev_payload)
        .send()
        .await
        .expect("Failed to call /player/cache/previous");
    assert_eq!(cache_prev_resp.status(), 202);

    sleep(Duration::from_secs(2)).await;

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(status_resp.status(), 200);
    let status_body: serde_json::Value = status_resp.json().await.expect("Failed to parse /player/status JSON");
    assert_eq!(status_body["next_cached"], true);
    assert_eq!(status_body["prev_cached"], true);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_cache_stream_upload_endpoints() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let next_audio = std::env::temp_dir().join("l337-test-cache-next-stream.wav");
    write_minimal_wav(&next_audio);
    let prev_audio = std::env::temp_dir().join("l337-test-cache-prev-stream.wav");
    write_minimal_wav(&prev_audio);
    let next_bytes = std::fs::read(&next_audio).expect("Failed to read next WAV bytes");
    let prev_bytes = std::fs::read(&prev_audio).expect("Failed to read prev WAV bytes");

    let next_resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/next/stream", _port))
        .header("Authorization", format!("Bearer {}", token))
        .header("X-Track-Id", "cache-next-stream")
        .header("X-Title", "Cached Next")
        .body(next_bytes)
        .send()
        .await
        .expect("Failed to call /player/cache/next/stream");
    assert_eq!(next_resp.status(), 200);

    let prev_resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/previous/stream", _port))
        .header("Authorization", format!("Bearer {}", token))
        .header("X-Track-Id", "cache-prev-stream")
        .header("X-Title", "Cached Prev")
        .body(prev_bytes)
        .send()
        .await
        .expect("Failed to call /player/cache/previous/stream");
    assert_eq!(prev_resp.status(), 200);

    let lookup_payload = serde_json::json!({
        "track_ids": ["cache-next-stream", "cache-prev-stream"]
    });

    let lookup_resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/lookup", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&lookup_payload)
        .send()
        .await
        .expect("Failed to call /player/cache/lookup");
    assert_eq!(lookup_resp.status(), 200);
    let lookup_body: serde_json::Value = lookup_resp.json().await.expect("Failed to parse /player/cache/lookup JSON");
    let cached: Vec<String> = lookup_body["cached"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert!(cached.contains(&"cache-next-stream".to_string()));
    assert!(cached.contains(&"cache-prev-stream".to_string()));

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_auth_challenge_redeem_happy_path() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status with redeemed token");

    assert_eq!(resp.status(), 200);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_auth_redeem_rejects_wrong_challenge() {
    let (_port, mut child, _config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let challenge_resp = client
        .post(format!("https://127.0.0.1:{}/auth/challenge", _port))
        .send()
        .await
        .expect("Failed to call /auth/challenge");
    assert_eq!(challenge_resp.status(), 202);

    let redeem_resp = client
        .post(format!("https://127.0.0.1:{}/auth/redeem", _port))
        .header("X-L337-Challenge", "wrong-token")
        .send()
        .await
        .expect("Failed to call /auth/redeem");

    assert_eq!(redeem_resp.status(), 401);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::test]
async fn test_auth_redeem_is_single_use() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");

    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let redeem_resp = client
        .post(format!("https://127.0.0.1:{}/auth/redeem", _port))
        .header("X-L337-Challenge", &token)
        .send()
        .await
        .expect("Failed to call /auth/redeem second time");

    assert_eq!(redeem_resp.status(), 401);

    let _ = child.kill().await;
    let _ = child.wait().await;
}
