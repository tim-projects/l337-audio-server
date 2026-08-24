#[cfg(test)]
mod tests {
    use crate::player::engine::{PlayerEngine, decode_to_pcm, resample_interleaved};
    use crate::player::storage::StorageManager;
    use std::path::PathBuf;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

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

    #[tokio::test]
    async fn test_player_engine_send_sync() {
        let storage = StorageManager::new(500 * 1024 * 1024, Some(PathBuf::from("./test_cache"))).await;
        let _engine = PlayerEngine::new_dummy(storage);
        assert_send::<PlayerEngine>();
        assert_sync::<PlayerEngine>();
    }

    #[tokio::test]
    async fn test_set_speed_clamping() {
        let storage = StorageManager::new(500 * 1024 * 1024, Some(PathBuf::from("./test_cache"))).await;
        let mut engine = PlayerEngine::new_dummy(storage);
        engine.set_speed(0.1);
        assert_eq!(engine.speed, 0.25);
        engine.set_speed(10.0);
        assert_eq!(engine.speed, 4.0);
        engine.set_speed(1.0);
        assert_eq!(engine.speed, 1.0);
    }

    #[tokio::test]
    async fn test_set_volume_clamping() {
        let storage = StorageManager::new(500 * 1024 * 1024, Some(PathBuf::from("./test_cache"))).await;
        let mut engine = PlayerEngine::new_dummy(storage);
        engine.set_volume(-0.5);
        assert_eq!(engine.volume_val, 0.0);
        engine.set_volume(1.5);
        assert_eq!(engine.volume_val, 1.0);
        engine.set_volume(0.5);
        assert_eq!(engine.volume_val, 0.5);
    }

    #[tokio::test]
    async fn test_seek_position_tracking() {
        let dir = std::env::temp_dir().join("l337-player-test");
        let _ = std::fs::create_dir_all(&dir);
        let storage = StorageManager::new(500 * 1024 * 1024, Some(dir.clone())).await;
        let mut engine = PlayerEngine::new_dummy(storage);

        let wav_path = dir.join("seek-unit-test.wav");
        write_minimal_wav(&wav_path);
        let slot_path = engine.storage.get_active_slot_path("current");
        let _ = std::fs::copy(&wav_path, &slot_path);

        engine.seek(0);
        let status = engine.get_status().await;
        assert_eq!(status.state, crate::api::models::PlayerStateLabel::Playing);
        assert!(status.position_sec.is_some());
        assert_eq!(status.position_sec.unwrap(), 0);
        assert!(status.duration_sec.is_some());
        assert_eq!(status.duration_sec.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_decode_to_pcm_wav() {
        let dir = std::env::temp_dir().join("l337-decode-test");
        let _ = std::fs::create_dir_all(&dir);
        let wav_path = dir.join("decode-unit-test.wav");
        write_minimal_wav(&wav_path);
        let bytes = std::fs::read(&wav_path).expect("Failed to read WAV bytes");

        let result = decode_to_pcm(bytes);
        assert!(result.is_ok(), "decode_to_pcm failed: {:?}", result.err());
        let (pcm, sample_rate, channels) = result.unwrap();
        assert!(!pcm.is_empty(), "PCM output should not be empty");
        assert_eq!(sample_rate, 44100);
        assert_eq!(channels, 2);
    }

    #[tokio::test]
    async fn test_resample_interleaved_identity() {
        let input: Vec<f32> = (0..48000)
            .flat_map(|i| {
                let sample = (i as f32 / 48000.0 * std::f32::consts::TAU * 440.0).sin();
                [sample, sample]
            })
            .collect();

        let output = resample_interleaved(&input, 2, 48000, 48000, 1.0);
        assert!(
            (output.len() as i64 - input.len() as i64).abs() <= input.len() as i64 / 50,
            "identity resample length {} != input {}",
            output.len(),
            input.len()
        );
        let input_rms: f32 = input.iter().map(|s| s * s).sum::<f32>() / input.len() as f32;
        let output_rms: f32 = output.iter().map(|s| s * s).sum::<f32>() / output.len() as f32;
        assert!(
            (input_rms - output_rms).abs() < 0.1,
            "identity resample RMS drift: input {} vs output {}",
            input_rms,
            output_rms
        );
    }

    #[tokio::test]
    async fn test_resample_interleaved_downsample() {
        let input: Vec<f32> = (0..48000)
            .flat_map(|i| {
                let sample = (i as f32 / 48000.0 * std::f32::consts::TAU * 440.0).sin();
                [sample, sample]
            })
            .collect();

        let output = resample_interleaved(&input, 2, 48000, 44100, 1.0);
        let expected_frames = (input.len() as f64 * 44100.0 / 48000.0) as usize / 2;
        assert!(
            (output.len() as i64 - (expected_frames * 2) as i64).abs() <= input.len() as i64 / 20,
            "downsample length {} != expected ~{}",
            output.len(),
            expected_frames * 2
        );
    }
}
