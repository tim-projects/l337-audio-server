use reqwest::Client;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, sleep};

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
/// Returns the port, a handle to the child process, and the state directory the
/// challenge token is written to.
async fn spawn_server() -> (u16, tokio::process::Child, PathBuf) {
    spawn_server_inner(None).await
}

/// Like `spawn_server`, but with `extra_path` prepended to the server's PATH —
/// the way a test puts a fake `yt-dlp` in front of a real one (the server
/// resolves yt-dlp by name from its own environment).
async fn spawn_server_with_path(extra_path: &PathBuf) -> (u16, tokio::process::Child, PathBuf) {
    spawn_server_inner(Some(extra_path)).await
}

async fn spawn_server_inner(extra_path: Option<&PathBuf>) -> (u16, tokio::process::Child, PathBuf) {
    // Per-spawn isolation, not a shared config dir: the server locks fixed
    // runtime paths (its runtime dir plus a hardcoded /tmp fallback candidate)
    // and writes its challenge token to STATE_DIRECTORY, so concurrently
    // spawned test servers would otherwise steal each other's single-instance
    // lock ("Another instance is already running") and read each other's
    // tokens. L337_RUNTIME_DIR moves every runtime path (socket + all lock
    // candidates) inside the per-spawn dir on every platform; TMPDIR covers
    // temp-dir lookups; STATE_DIRECTORY is where the server actually resolves
    // the challenge token from (it never reads XDG_CONFIG_HOME).
    static SPAWN_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SPAWN_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let base_dir = std::env::temp_dir().join(format!("l337-test-{}-{seq}", std::process::id()));
    let config_dir = base_dir.join("l337-audio-server");
    let _ = std::fs::create_dir_all(&config_dir);

    let port = find_available_port().await;

    // Cargo sets this for integration tests to the binary it just built in the
    // target dir; path-guessing (a repo bin/ dir, or PATH) only finds a released
    // binary, which may not exist on a fresh checkout or a device.
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_l337-audio-server"));

    let mut child = Command::new(binary_path)
        .arg("--dummy")
        .arg("--transport=http")
        .env("L337__SERVER__PORT", port.to_string())
        .env("L337__SERVER__HOST", "127.0.0.1")
        .env("TMPDIR", &base_dir)
        .envs(extra_path.map(|dir| {
            (
                "PATH".to_string(),
                format!(
                    "{}:{}",
                    dir.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
        }))
        .env("L337_RUNTIME_DIR", &base_dir)
        // The server resolves the challenge token's directory from
        // STATE_DIRECTORY (falling back to the platform cache dir), never from
        // XDG_CONFIG_HOME — point it at this test's isolated dir so the token
        // lands exactly where get_token_via_challenge() reads it.
        .env("STATE_DIRECTORY", &config_dir)
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
    // 180s, not 60: the whole suite runs these in parallel, and on a slow device
    // (a phone running 22 servers at once, each generating its TLS cert) the
    // default budget expires under CPU contention.
    for _ in 0..360 {
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

/// Spawn the throttled HTTP server (scripts/throttle_server.py) serving a local
/// file at a slow byte rate, to reproduce the MKV demuxer wedge without
/// YouTube rate limits. Returns the child process and the port it listens on.
///
/// Only compiled with `--features opus` because WebM/Opus decoding requires
/// the Opus codec.
#[cfg(feature = "opus")]
async fn spawn_throttle_server(fixture: &PathBuf) -> (u16, tokio::process::Child) {
    let port = 18080u16;
    let mut child = Command::new("python3")
        .args(&[
            "scripts/throttle_server.py",
            "--file", fixture.to_str().unwrap(),
            "--rate", "32768",
            "--port", &port.to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn throttled HTTP server");

    // Wait for the throttle server to be ready by polling /size.
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");
    let url = format!("http://127.0.0.1:{port}/size");
    for _ in 0..40 {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return (port, child);
            }
        }
        sleep(Duration::from_millis(250)).await;
    }
    let _ = child.kill().await;
    panic!("Throttled HTTP server did not become ready on port {port}");
}

/// MKV demuxer wedge (finding U2/U7 follow-up): streaming a WebM/Opus file over
/// a throttled connection triggers Symphonia's "past master element" error when
/// an element crosses the probe-time size boundary in a partially-downloaded
/// file. The reader rebuilds but stays wedged because wait_for_progress()
/// waits for download growth that may never come (download already complete).
///
/// Reproduces locally without YouTube rate limits: a Python throttled server
/// serves the WebM fixture at ~32 KB/s, and the l337 server streams it via the
/// direct-stream URL path (googlevideo.com/videoplayback). position_sec must
/// advance beyond 0.
#[cfg(feature = "opus")]
#[tokio::test]
async fn test_mkv_wedge_streaming() {
    let fixture = std::env::temp_dir().join("l337-test-fixture-251.webm");
    let fixture_source =
        std::env::current_dir().unwrap().join("tests/fixtures/test_251.webm");
    std::fs::copy(&fixture_source, &fixture).expect("Failed to copy test fixture");

    let (_throttle_port, mut throttle_child) = spawn_throttle_server(&fixture).await;

    let (_port, mut l337_child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");
    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    // Play the WebM stream via the throttled direct-stream URL.
    // The path contains "googlevideo.com/videoplayback" so
    // is_direct_stream_url() matches and the server streams (not downloads).
    let stream_url =
        "http://127.0.0.1:18080/googlevideo.com/videoplayback/test_251.webm";

    let resp = client
        .post(format!("https://127.0.0.1:{_port}/player/play"))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "track_id": "mkv-wedge-test",
            "stream_url": stream_url,
            "duration": 30
        }))
        .send()
        .await
        .expect("Failed to call /player/play");

    assert_eq!(resp.status(), 200);

    // Poll position_sec for up to 30 seconds — it must advance beyond 0.
    let mut position_advanced = false;
    for _ in 0..60 {
        let status_resp = client
            .get(format!("https://127.0.0.1:{_port}/player/status"))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .expect("Failed to call /player/status");
        let status = status_resp.json::<serde_json::Value>().await.unwrap();
        let pos = status["position_sec"].as_u64().unwrap_or(0);
        let state = status["state"].as_str().unwrap_or("");
        if pos > 0 {
            position_advanced = true;
            break;
        }
        if state == "stopped" {
            break; // End of track reached without advancing
        }
        sleep(Duration::from_millis(500)).await;
    }

    assert!(
        position_advanced,
        "MKV demuxer wedge: position_sec stayed at 0 during streaming playback"
    );

    let _ = l337_child.kill().await;
    let _ = l337_child.wait().await;
    let _ = throttle_child.kill().await;
    let _ = throttle_child.wait().await;
    let _ = std::fs::remove_file(&fixture);
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

    // Capabilities must advertise what the binary can actually do, so a
    // front-end can drop its "assume AAC only" stopgap and offer quality presets.
    let caps = &body["capabilities"];
    assert!(caps["yt_dlp"].is_boolean());
    let codecs = caps["codecs"].as_array().expect("capabilities.codecs");
    let codecs: Vec<&str> = codecs.iter().filter_map(|c| c.as_str()).collect();
    assert!(
        codecs.contains(&"aac") && codecs.contains(&"pcm_s16le"),
        "unexpected advertised codecs: {codecs:?}"
    );
    // Opus is compile-time optional: advertised only when it is linked in.
    #[cfg(feature = "opus")]
    assert!(
        codecs.contains(&"opus"),
        "opus is linked in but not advertised"
    );
    #[cfg(not(feature = "opus"))]
    assert!(
        !codecs.contains(&"opus"),
        "opus is not linked in but is advertised"
    );
    assert_eq!(caps["quality"], true);

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
    let body: serde_json::Value = resp
        .json()
        .await
        .expect("Failed to parse /player/status JSON");
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
    let body: serde_json::Value = resp
        .json()
        .await
        .expect("Failed to parse /player/play JSON");
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
    let body: serde_json::Value = resp
        .json()
        .await
        .expect("Failed to parse /player/cache/lookup JSON");
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
        "position_sec": 0
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
    let status_body: serde_json::Value = status_resp
        .json()
        .await
        .expect("Failed to parse /player/status JSON");
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
    let status_body: serde_json::Value = status_resp
        .json()
        .await
        .expect("Failed to parse /player/status JSON");
    assert_eq!(status_body["state"], "playing");
    assert_eq!(
        status_body["current_track"]["track_id"],
        "stream-test-track"
    );
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
    let status_body: serde_json::Value = status_resp
        .json()
        .await
        .expect("Failed to parse /player/status JSON");
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
    let status_body: serde_json::Value = status_resp
        .json()
        .await
        .expect("Failed to parse /player/status JSON");
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
    assert_eq!(cache_next_resp.status(), 200);

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
    assert_eq!(cache_prev_resp.status(), 200);

    sleep(Duration::from_secs(2)).await;

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(status_resp.status(), 200);
    let status_body: serde_json::Value = status_resp
        .json()
        .await
        .expect("Failed to parse /player/status JSON");
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
        .post(format!(
            "https://127.0.0.1:{}/player/cache/next/stream",
            _port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("X-Track-Id", "cache-next-stream")
        .header("X-Title", "Cached Next")
        .body(next_bytes)
        .send()
        .await
        .expect("Failed to call /player/cache/next/stream");
    assert_eq!(next_resp.status(), 200);

    let prev_resp = client
        .post(format!(
            "https://127.0.0.1:{}/player/cache/previous/stream",
            _port
        ))
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
    let lookup_body: serde_json::Value = lookup_resp
        .json()
        .await
        .expect("Failed to parse /player/cache/lookup JSON");
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

/// A fake `yt-dlp` for the PATH-prefixed spawn: it serves a fixed audio-only
/// rendition ladder to `--dump-single-json`, and for the actual download records
/// its argv (so the test can see which rendition the server selected) before
/// writing the fixture WAV to yt-dlp's `-o` destination.
///
/// `#!/bin/sh`, not bash: /bin/bash does not exist on Termux (/bin is
/// /system/bin) and a missing interpreter makes execvp report ENOENT, which
/// would silently fall through to a real yt-dlp further down PATH.
fn write_fake_ytdlp(dir: &PathBuf, fixture: &PathBuf, log: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    // The ladder yt-dlp reports for a typical YouTube video: a high Opus tier
    // above any test ceiling, plus AAC and two Opus tiers below it.
    let script = format!(
        r#"#!/bin/sh
case "$*" in
  *--version*)
    echo "fake yt-dlp (test fixture)"; exit 0 ;;
esac
case "$*" in
  *--dump-single-json*)
    cat <<'JSON'
{{"duration": 10, "formats": [
  {{"format_id": "251", "abr": 158.0, "acodec": "opus",  "vcodec": "none", "ext": "webm", "duration": 10, "url": "https://example.invalid/251"}},
  {{"format_id": "140", "abr": 129.0, "acodec": "mp4a.40.2", "vcodec": "none", "ext": "m4a", "duration": 10, "url": "https://example.invalid/140"}},
  {{"format_id": "250", "abr": 74.0,  "acodec": "opus",  "vcodec": "none", "ext": "webm", "duration": 10, "url": "https://example.invalid/250"}},
  {{"format_id": "249", "abr": 51.0,  "acodec": "opus",  "vcodec": "none", "ext": "webm", "duration": 10, "url": "https://example.invalid/249"}}
]}}
JSON
    exit 0 ;;
esac
# The download invocation: record it, then produce the fixture.
echo "$@" >> "{log}"
dest=""
while [ $# -gt 0 ]; do
  if [ "$1" = "-o" ]; then dest="$2"; fi
  shift
done
if [ -n "$dest" ]; then cp "{fixture}" "$dest"; fi
exit 0
"#,
        log = log.display(),
        fixture = fixture.display()
    );
    std::fs::write(dir.join("yt-dlp"), script).unwrap();
    let _ = std::fs::set_permissions(dir.join("yt-dlp"), std::fs::Permissions::from_mode(0o755));
}

/// G2 / finding #4: after `POST /player/target {"target_kbps":64}` the
/// *background cache* must resolve its rendition under the runtime ceiling
/// (not the config default). The fake yt-dlp records the format id the server
/// asked for: the highest tier at or below 64 kbps (249, 51 kbps) — never the
/// 158 kbps "best" tier the pre-fix code picked with the config default.
#[tokio::test]
async fn test_cache_next_honors_runtime_target_ceiling() {
    let fake_dir = std::env::temp_dir().join("l337-test-fake-ytdlp");
    let _ = std::fs::create_dir_all(&fake_dir);
    let fixture = fake_dir.join("fixture.wav");
    write_minimal_wav(&fixture);
    let invocations = fake_dir.join("invocations.log");
    let _ = std::fs::remove_file(&invocations);
    write_fake_ytdlp(&fake_dir, &fixture, &invocations);

    let (_port, mut child, config_dir) = spawn_server_with_path(&fake_dir).await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");
    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    // Set the runtime ceiling, then confirm it is what the server reports.
    let target_resp = client
        .post(format!("https://127.0.0.1:{}/player/target", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({"target_kbps": 64}))
        .send()
        .await
        .expect("Failed to call /player/target");
    assert_eq!(target_resp.status(), 200);
    assert_eq!(
        target_resp.json::<serde_json::Value>().await.unwrap()["target_kbps"],
        64
    );

    // (No status check here: `PlayerStatus.target_kbps` is the *effective*
    // per-track ceiling, which is 0 until a track actually plays under the new
    // runtime default — the setter's own response is the echo of that default.)

    // Background cache of a YouTube track under that ceiling.
    let cache_resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/next", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "track_id": "yt-ceiling-next",
            "stream_url": "https://www.youtube.com/watch?v=ceilingtest",
            "duration": 10
        }))
        .send()
        .await
        .expect("Failed to call /player/cache/next");
    assert_eq!(cache_resp.status(), 200);

    // The cache task runs in the background: wait for the download invocation.
    let mut recorded = String::new();
    for _ in 0..120 {
        if let Ok(contents) = std::fs::read_to_string(&invocations) {
            if contents.contains("-f ") {
                recorded = contents;
                break;
            }
        }
        sleep(Duration::from_millis(500)).await;
    }
    assert!(
        recorded.contains("-f 249"),
        "cache_next downloaded a rendition above the 64 kbps runtime ceiling: {recorded:?}"
    );
    assert!(
        !recorded.contains("-f 251") && !recorded.contains("-f 140"),
        "cache_next ignored the runtime ceiling and picked a higher tier: {recorded:?}"
    );

    // The cached slot must also reach the client-visible state.
    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(
        status_resp.json::<serde_json::Value>().await.unwrap()["next_cached"],
        true
    );

    let _ = child.kill().await;
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&fake_dir);
}

/// G2 / finding #4: with no runtime ceiling (`target_kbps` unset → 0 == best),
/// the background cache must ask yt-dlp for the **highest available** audio
/// rendition — the 158 k Opus (251) — rather than whatever yt-dlp's own
/// `bestaudio` ranking would have picked (a ~129 k AAC m4a, itag 140).
#[tokio::test]
async fn test_cache_next_best_picks_highest_available() {
    let fake_dir = std::env::temp_dir().join("l337-test-fake-ytdlp-best");
    let _ = std::fs::create_dir_all(&fake_dir);
    let fixture = fake_dir.join("fixture.wav");
    write_minimal_wav(&fixture);
    let invocations = fake_dir.join("invocations.log");
    let _ = std::fs::remove_file(&invocations);
    write_fake_ytdlp(&fake_dir, &fixture, &invocations);

    let (_port, mut child, config_dir) = spawn_server_with_path(&fake_dir).await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");
    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    // No runtime ceiling: the default `default_target_kbps = 0` means best.
    let cache_resp = client
        .post(format!("https://127.0.0.1:{}/player/cache/next", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "track_id": "yt-best-next",
            "stream_url": "https://www.youtube.com/watch?v=besttest",
            "duration": 10
        }))
        .send()
        .await
        .expect("Failed to call /player/cache/next");
    assert_eq!(cache_resp.status(), 200);

    // The cache task runs in the background: wait for the download invocation.
    let mut recorded = String::new();
    for _ in 0..120 {
        if let Ok(contents) = std::fs::read_to_string(&invocations) {
            if contents.contains("-f ") {
                recorded = contents;
                break;
            }
        }
        sleep(Duration::from_millis(500)).await;
    }
    assert!(
        recorded.contains("-f 251"),
        "best must request the highest available rendition (251): {recorded:?}"
    );
    assert!(
        !recorded.contains("-f 140"),
        "best must not fall back to yt-dlp's lower AAC ranking (140): {recorded:?}"
    );

    let _ = child.kill().await;
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&fake_dir);
}
/// Test /player/session/connect and /player/session/disconnect: exercises
/// session lifecycle used by the TUI player for warm-up/cool-down accounting.
/// Also verifies status exposes warm/active_sessions fields.
#[tokio::test]
async fn test_session_connect_disconnect_and_status() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");
    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let connect = client
        .post(format!(
            "https://127.0.0.1:{}/player/session/connect",
            _port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/session/connect");
    assert_eq!(connect.status(), 200);
    let connect_json = connect.json::<serde_json::Value>().await.unwrap();
    assert_eq!(connect_json["ok"], true);
    assert!(connect_json["active_sessions"].as_u64().unwrap_or(0) >= 1);

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    assert_eq!(status_resp.status(), 200);
    let status = status_resp.json::<serde_json::Value>().await.unwrap();
    assert!(status.get("warm").is_some(), "status must expose warm");
    assert!(
        status.get("active_sessions").is_some(),
        "status must expose active_sessions"
    );
    assert!(status["active_sessions"].as_u64().unwrap_or(0) >= 1);

    let disconnect = client
        .post(format!(
            "https://127.0.0.1:{}/player/session/disconnect",
            _port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/session/disconnect");
    assert_eq!(disconnect.status(), 200);
    let disconnect_json = disconnect.json::<serde_json::Value>().await.unwrap();
    assert_eq!(disconnect_json["ok"], true);

    let status2 = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status")
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(status2["active_sessions"].as_u64().unwrap_or(99), 0);

    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// — it is skipped and the server default applies (`resolve_ceiling` doc
/// contract). Uploaded through the real endpoint against a real file source, so
/// this covers the axum `Json` extractor's tolerant deserializer, not just the
/// unit test in models.rs.
#[tokio::test]
async fn test_play_unknown_quality_falls_back_to_server_default() {
    let (_port, mut child, config_dir) = spawn_server().await;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client");
    let token = get_token_via_challenge(&client, _port, &config_dir).await;

    let audio_path = std::env::temp_dir().join("l337-test-quality.wav");
    write_minimal_wav(&audio_path);

    let payload = serde_json::json!({
        "track_id": "quality-ultra-track",
        "stream_url": format!("file://{}", audio_path.display()),
        "quality": "ultra",
        "duration": 1
    });

    let resp = client
        .post(format!("https://127.0.0.1:{}/player/play", _port))
        .header("Authorization", format!("Bearer {}", token))
        .json(&payload)
        .send()
        .await
        .expect("Failed to call /player/play");

    // 200, not 422: the unknown label is skipped, falling back to the server
    // default (here the unset default: 0 == best).
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<serde_json::Value>().await.unwrap()["ok"], true);

    let status_resp = client
        .get(format!("https://127.0.0.1:{}/player/status", _port))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to call /player/status");
    let status = status_resp.json::<serde_json::Value>().await.unwrap();
    assert_eq!(
        status["target_kbps"], 0,
        "unknown quality must fall back to the server default"
    );

    let _ = child.kill().await;
    let _ = child.wait().await;
}
