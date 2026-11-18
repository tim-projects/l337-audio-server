use std::os::raw::{c_char, c_int, c_double, c_long, c_short};
use std::path::Path;
use tracing::{info, warn};

#[repr(C)]
pub struct VorbisDecoder {
    _private: [u8; 0],
}

// Native libvorbis FFI (available when `vorbis` feature is enabled and native library linked)
#[cfg(feature = "vorbis")]
unsafe extern "C" {
    fn vorbis_decoder_open(path: *const c_char) -> *mut VorbisDecoder;
    fn vorbis_decoder_get_info(
        dec: *mut VorbisDecoder,
        out_channels: *mut c_int,
        out_sample_rate: *mut c_int,
        out_duration: *mut c_double,
        out_total_samples: *mut c_long,
    ) -> c_int;
    fn vorbis_decoder_read_chunk(
        dec: *mut VorbisDecoder,
        out_buffer: *mut f32,
        max_samples: c_int,
        out_samples_read: *mut c_int,
    ) -> c_int;
    fn vorbis_decoder_get_time(dec: *mut VorbisDecoder) -> c_double;
    fn vorbis_decoder_get_pcm_pos(dec: *mut VorbisDecoder) -> c_long;
    fn vorbis_decoder_seek_pcm(dec: *mut VorbisDecoder, pcm_pos: c_long) -> c_int;
    fn vorbis_decoder_seek_time(dec: *mut VorbisDecoder, time_sec: c_double) -> c_int;
    fn vorbis_decoder_close(dec: *mut VorbisDecoder);
    fn decode_vorbis_file_c(
        path: *const c_char,
        out_data: *mut *mut f32,
        out_len: *mut c_int,
        out_channels: *mut c_int,
        out_sample_rate: *mut c_int,
        out_duration: *mut c_double,
    ) -> c_int;
    fn free_vorbis_result(data: *mut f32);
}

#[cfg(feature = "vorbis")]
pub struct VorbisDecoderHandle {
    ptr: *mut VorbisDecoder,
}

#[cfg(feature = "vorbis")]
impl VorbisDecoderHandle {
    pub fn open(path: &Path) -> Result<Self, String> {
        let path_cstr = std::ffi::CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let ptr = unsafe { vorbis_decoder_open(path_cstr.as_ptr()) };
        if ptr.is_null() {
            return Err("Failed to open Vorbis decoder (native library not available)".into());
        }
        Ok(Self { ptr })
    }

    pub fn get_info(&self) -> Result<(u16, u32, f64, i64), String> {
        let mut channels: c_int = 0;
        let mut sample_rate: c_int = 0;
        let mut duration: c_double = 0.0;
        let mut total_samples: c_long = 0;
        
        let ret = unsafe {
            vorbis_decoder_get_info(self.ptr, &mut channels, &mut sample_rate, &mut duration, &mut total_samples)
        };
        if ret < 0 {
            return Err("Failed to get Vorbis info".into());
        }
        Ok((channels as u16, sample_rate as u32, duration, total_samples as i64))
    }

    pub fn read_chunk(&mut self, buffer: &mut [f32]) -> Result<usize, String> {
        let mut samples_read: c_int = 0;
        let ret = unsafe {
            vorbis_decoder_read_chunk(self.ptr, buffer.as_mut_ptr(), buffer.len() as c_int, &mut samples_read)
        };
        if ret < 0 {
            return Err("Vorbis read error".into());
        }
        Ok(samples_read as usize)
    }

    /// Get the current decode position in seconds using Vorbisfile's ov_time_tell
    pub fn get_time(&self) -> f64 {
        unsafe { vorbis_decoder_get_time(self.ptr) }
    }

    /// Get the current decode position in PCM samples using Vorbisfile's ov_pcm_tell
    pub fn get_pcm_pos(&self) -> i64 {
        unsafe { vorbis_decoder_get_pcm_pos(self.ptr) }
    }

    /// Seek to a specific PCM sample position
    pub fn seek_pcm(&mut self, pcm_pos: i64) -> Result<(), String> {
        let ret = unsafe { vorbis_decoder_seek_pcm(self.ptr, pcm_pos) };
        if ret < 0 {
            return Err("Vorbis PCM seek failed".into());
        }
        Ok(())
    }

    /// Seek to a specific time position in seconds
    pub fn seek_time(&mut self, time_sec: f64) -> Result<(), String> {
        let ret = unsafe { vorbis_decoder_seek_time(self.ptr, time_sec) };
        if ret < 0 {
            return Err("Vorbis time seek failed".into());
        }
        Ok(())
    }
}

#[cfg(feature = "vorbis")]
impl Drop for VorbisDecoderHandle {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { vorbis_decoder_close(self.ptr) };
            self.ptr = std::ptr::null_mut();
        }
    }
}

/// Decode an Ogg Vorbis file to interleaved f32 PCM at native sample rate (one-shot).
/// Returns (pcm_samples, sample_rate, channels, duration_seconds).
///
/// Tries native libvorbis FFI first (if `vorbis` feature enabled and library available),
/// falls back to Symphonia pure-Rust decoder.
pub fn decode_vorbis_file(path: &Path) -> Result<(Vec<f32>, u32, u16, f64), String> {
    // Try native libvorbis FFI first
    #[cfg(feature = "vorbis")]
    {
        info!("vorbis: decode_vorbis_file start (trying native libvorbis FFI)");
        let path_cstr = std::ffi::CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;

        let mut out_data: *mut f32 = std::ptr::null_mut();
        let mut out_len: c_int = 0;
        let mut out_channels: c_int = 0;
        let mut out_sample_rate: c_int = 0;
        let mut out_duration: f64 = 0.0;

        let ret = unsafe {
            decode_vorbis_file_c(
                path_cstr.as_ptr(),
                &mut out_data,
                &mut out_len,
                &mut out_channels,
                &mut out_sample_rate,
                &mut out_duration,
            )
        };

        if ret >= 0 && !out_data.is_null() {
            let len = out_len as usize;
            let channels = out_channels as u16;
            let sample_rate = out_sample_rate as u32;
            let duration = out_duration;

            // Copy the C-allocated data into a Rust-owned Vec to avoid allocator mismatch
            let pcm = unsafe { std::slice::from_raw_parts(out_data, len).to_vec() };
            
            // Free the C-allocated memory
            unsafe { free_vorbis_result(out_data) };

            info!("vorbis: native decode succeeded ({} samples, {} Hz, {} ch)", len, sample_rate, channels);
            return Ok((pcm, sample_rate, channels, duration));
        }
        
        warn!("vorbis: native decode failed or unavailable, falling back to Symphonia");
    }

    // Fallback to Symphonia pure-Rust Vorbis decoder
    #[cfg(all(feature = "vorbis", feature = "symphonia"))]
    {
        info!("vorbis: decode_vorbis_file start (Symphonia fallback)");
        decode_vorbis_file_symphonia(path)
    }

    // No decoder available
    #[cfg(not(all(feature = "vorbis", feature = "symphonia")))]
    {
        Err("No Vorbis decoder available (enable 'vorbis' feature for native or 'symphonia' for fallback)".into())
    }
}

/// Decode Vorbis using Symphonia (pure Rust)
#[cfg(all(feature = "vorbis", feature = "symphonia"))]
fn decode_vorbis_file_symphonia(path: &Path) -> Result<(Vec<f32>, u32, u16, f64), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let source = Box::new(BufReader::new(file));
    let mss = MediaSourceStream::new(source, Default::default());

    let hint = Hint::new();
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Symphonia probe failed: {}", e))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec == symphonia::core::codecs::CODEC_TYPE_VORBIS)
        .ok_or("No Vorbis track found")?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Symphonia decoder create failed: {}", e))?;

    let track_id = track.id;
    let mut all_samples = Vec::new();
    let mut total_frames = 0u64;
    let file_sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.unwrap_or(symphonia::core::audio::Channels::STEREO).count() as u16;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("Symphonia packet error: {}", e)),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sample_buf.copy_interleaved_ref(decoded);
                total_frames += decoded.frames() as u64;
                all_samples.extend_from_slice(sample_buf.samples());
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Symphonia decode error: {}", e)),
        }
    }

    let duration = total_frames as f64 / file_sample_rate as f64;
    Ok((all_samples, file_sample_rate, channels, duration))
}

#[cfg(test)]
mod tests {
    use super::*;
}