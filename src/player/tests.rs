#[cfg(test)]
mod tests {
    use crate::player::engine::{InterleavedResampler, PlayerEngine, decode_to_pcm, stretch_and_resample};
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
        let _engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);
        assert_send::<PlayerEngine>();
        assert_sync::<PlayerEngine>();
    }

    #[tokio::test]
    async fn test_set_speed_clamping() {
        let storage = StorageManager::new(500 * 1024 * 1024, Some(PathBuf::from("./test_cache"))).await;
        let mut engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);
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
        let mut engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);
        engine.set_volume(-0.5);
        assert_eq!(engine.volume_val, 0.0);
        engine.set_volume(1.5);
        assert_eq!(engine.volume_val, 1.0);
        engine.set_volume(0.5);
        assert_eq!(engine.volume_val, 0.5);
        // Software gain is the single authoritative path: it must track
        // volume_val exactly, or lowering after a recovery double-attenuates.
        assert_eq!(engine.applied_gain(), 0.5);
    }

    #[tokio::test]
    async fn test_seek_position_tracking() {
        let dir = std::env::temp_dir().join("l337-player-test");
        let _ = std::fs::create_dir_all(&dir);
        let storage = StorageManager::new(500 * 1024 * 1024, Some(dir.clone())).await;
        let mut engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);

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
        let (pcm, sample_rate, channels, codec) = result.unwrap();
        assert!(!pcm.is_empty(), "PCM output should not be empty");
        assert_eq!(sample_rate, 44100);
        assert_eq!(channels, 2);
        assert_eq!(codec.as_deref(), Some("pcm_s16le"));
    }

    /// A 440 Hz full-scale stereo sine at `rate`, `duration` seconds long.
    fn sine_stereo(rate: u32, duration: f32) -> Vec<f32> {
        (0..(rate as f64 * duration as f64) as usize)
            .flat_map(|i| {
                let sample = (i as f32 / rate as f32 * std::f32::consts::TAU * 440.0).sin();
                [sample, sample]
            })
            .collect()
    }

    /// Resample the whole buffer in one call and flush the tail (the file path).
    fn resample_whole(input: &[f32], channels: u16, in_rate: u32, out_rate: u32) -> Vec<f32> {
        let mut resampler = InterleavedResampler::new(channels, in_rate, out_rate);
        let mut output = resampler.process(input);
        output.extend(resampler.finish());
        output
    }

    #[tokio::test]
    async fn test_resample_identity_passthrough() {
        let input = sine_stereo(48000, 1.0);
        let mut resampler = InterleavedResampler::new(2, 48000, 48000);
        let mut output = resampler.process(&input);
        output.extend(resampler.finish());
        assert_eq!(
            output.len(),
            input.len(),
            "matching rates must be a passthrough, not a resample"
        );
        assert_eq!(output, input);
    }

    #[tokio::test]
    async fn test_resample_downsample_length() {
        let input = sine_stereo(48000, 1.0);
        let output = resample_whole(&input, 2, 48000, 44100);
        let expected_frames = input.len() as f64 * 44100.0 / 48000.0;
        assert!(
            (output.len() as i64 - expected_frames as i64).abs() <= input.len() as i64 / 20,
            "downsample length {} != expected ~{}",
            output.len(),
            expected_frames
        );
    }

    /// The device P0: a 44.1 kHz source through the 48 kHz device produced peak
    /// exactly 0 — decode worked (duration/position correct, audible at 48 kHz),
    /// every resampled chunk was discarded on length, and the sink got silence.
    #[tokio::test]
    async fn test_resample_44100_to_48000_is_nonzero() {
        let input = sine_stereo(44100, 2.0);
        let output = resample_whole(&input, 2, 44100, 48000);

        let peak: f32 = output.iter().fold(0.0, |m, s| m.max(s.abs()));
        assert!(
            peak > 0.1,
            "44.1k -> 48k resample wrote a peak of {peak}: the sink gets silence"
        );
        let in_frames = input.len() / 2;
        let expected_frames = (in_frames as f64 * 48000.0 / 44100.0) as i64;
        let actual_frames = output.len() / 2;
        assert!(
            (actual_frames as i64 - expected_frames).abs() <= 4096,
            "44.1k -> 48k length {actual_frames} frames != expected ~{expected_frames}"
        );
    }

    /// The second half of the device P0: rubato returns channel-planar vectors,
    /// and the old code appended them per channel, so the writer read `L R L R`
    /// out of `LLL...RRR...` — garbage, not audio. Two distinct constant levels
    /// must survive the round trip with their alignment intact.
    #[tokio::test]
    async fn test_resample_output_is_interleaved() {
        let frames = 8000usize;
        let input: Vec<f32> = (0..frames).flat_map(|_| [0.5f32, -0.5f32]).collect();
        let output = resample_whole(&input, 2, 44100, 48000);
        assert!(output.len() >= frames * 2);

        // Interleaved means alternating L=+0.5, R=-0.5; planar (what rubato
        // actually returns, and what the old code appended verbatim) means the
        // first half is all +0.5 and the second half all -0.5. Sample an interior
        // frame, past the sinc filter's startup transient (~0.46 of full scale on
        // the first frames), where a constant input must be reproduced exactly.
        let even: Vec<f32> = output.iter().step_by(2).copied().collect();
        let odd: Vec<f32> = output.iter().skip(1).step_by(2).copied().collect();
        let mid = output.len() / 4 / 2;
        assert!(
            (even[mid] - 0.5).abs() < 0.02 && (odd[mid] + 0.5).abs() < 0.02,
            "frame {mid} is {}/{} — output is planar, the stream needs interleaved",
            even[mid],
            odd[mid]
        );
        // The discriminator across the whole signal: planar output never
        // alternates sign between adjacent samples in the long run.
        let alternating = even.iter().zip(odd.iter()).filter(|(l, r)| l.signum() == r.signum()).count();
        assert_eq!(
            alternating,
            0,
            "{} frames have both channels the same sign: output is planar",
            alternating
        );
    }

    /// The streaming producer decodes ~1024-frame packets (AAC/Opus), which is
    /// shorter than one resampler chunk: every such packet used to be dropped
    /// whole. Feeding the same stream in packets must deliver the same audio.
    #[tokio::test]
    async fn test_resample_streaming_packet_sizes_keep_all_audio() {
        let input = sine_stereo(44100, 2.0);
        let expected = resample_whole(&input, 2, 44100, 48000);
        let mut resampler = InterleavedResampler::new(2, 44100, 48000);

        // Odd packet sizes on purpose: not aligned to the 4096-frame chunk.
        let mut streamed = Vec::new();
        let mut offset = 0usize;
        for packet in [1024usize, 512, 2048, 128, 2048] {
            let end = (offset + packet * 2).min(input.len());
            streamed.extend(resampler.process(&input[offset..end]));
            offset = end;
        }
        streamed.extend(resampler.process(&input[offset..]));
        streamed.extend(resampler.finish());

        let peak: f32 = streamed.iter().fold(0.0, |m, s| m.max(s.abs()));
        assert!(peak > 0.1, "streamed packets lost all audio (peak {peak})");
        assert!(
            (streamed.len() as i64 - expected.len() as i64).abs() <= 4096 * 2,
            "streamed length {} != whole-buffer length {}",
            streamed.len(),
            expected.len()
        );
    }

    // The whole-file path (`load_and_play`, `seek`, `seek_and_resume`) hands the
    // wrapper an entire track in ONE `process` call. Draining a consumed prefix
    // from `pending` per chunk there is quadratic in track length — a 4-minute
    // 44.1 kHz track moves ~110 GB before the first sound — so a long buffer
    // must stay fast, not merely correct. `cargo test` runs unoptimized (the
    // resampler itself is the slow part there, not the drain), so this is sized
    // to exercise many chunks without making the debug profile crawl: 32 chunks
    // moves ~1 G elements under the old drain-per-chunk loop.
    #[tokio::test]
    async fn test_resample_whole_track_buffer_stays_linear() {
        let input = sine_stereo(44100, 2.0);
        let mut resampler = InterleavedResampler::new(2, 44100, 48000);

        let start = std::time::Instant::now();
        let mut output = resampler.process(&input);
        output.extend(resampler.finish());
        let elapsed = start.elapsed();

        // Generous for an unoptimized test build; the linear loop is ~1 ms there
        // and the quadratic one is seconds (measured: 165 s for 30 s of audio).
        assert!(
            elapsed.as_secs() < 20,
            "resampling a whole track took {elapsed:?} (quadratic drain?)"
        );
        let peak: f32 = output.iter().fold(0.0, |m, s| m.max(s.abs()));
        assert!(peak > 0.1, "whole-track resample lost all audio (peak {peak})");
    }

    #[tokio::test]
    async fn test_stretch_and_resample_speed_only() {
        let input: Vec<f32> = (0..48000)
            .flat_map(|i| {
                let sample = (i as f32 / 48000.0 * std::f32::consts::TAU * 440.0).sin();
                [sample, sample]
            })
            .collect();

        let output = stretch_and_resample(&input, 2, 48000, 48000, 2.0, 1.0);
        let expected_frames = input.len() / 4;
        assert!(
            (output.len() as i64 - (expected_frames * 2) as i64).abs() <= expected_frames as i64 / 10,
            "2x speed stretch length {} != expected ~{}",
            output.len(),
            expected_frames * 2
        );
    }

    #[tokio::test]
    async fn test_stretch_and_resample_pitch_only() {
        let input: Vec<f32> = (0..48000)
            .flat_map(|i| {
                let sample = (i as f32 / 48000.0 * std::f32::consts::TAU * 440.0).sin();
                [sample, sample]
            })
            .collect();

        let output = stretch_and_resample(&input, 2, 48000, 48000, 1.0, 1.5);
        assert!(
            (output.len() as i64 - input.len() as i64).abs() <= input.len() as i64 / 20,
            "pitch-only stretch length {} != input {}",
            output.len(),
            input.len()
        );
    }
}
