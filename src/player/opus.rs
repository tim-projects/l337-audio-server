//! Opus decoding via direct libopus FFI.
//!
//! Why this exists: Symphonia ships **no Opus decoder** — its `symphonia-codec-opus`
//! crate is an unimplemented placeholder (see G1). Symphonia still *demuxes* Opus
//! containers (WebM/Matroska, Ogg, fragmented MP4) and hands us the Opus packets
//! plus the OpusHead side-band (`codec_params.extra_data`). We link the real
//! libopus and decode here — mirroring how the PulseAudio backend links libpulse
//! at build time in `build.rs` (no `dlopen`; a dlopen'd libpulse fails on bionic).
//!
//! Only the FFI subset the engine needs is declared. Opus always decodes at
//! 48 kHz; channel counts 1–2 (mono/stereo, i.e. everything YouTube serves) are
//! supported via the single-stream API. Multichannel (>2, channel mapping
//! family != 0) is intentionally not wired: it needs the multistream API and a
//! channel-mapping table, which no served source uses.
//! shortcut: >2-channel Opus rejected, add opus_multistream_* if a source emits it.

use std::os::raw::c_char;
use std::os::raw::c_int;

#[allow(non_camel_case_types)]
type opus_int32 = c_int;

#[repr(C)]
pub struct OpusDecoder(std::ffi::c_void);

unsafe extern "C" {
    /// Creates a decoder for a raw Opus stream of `Fs` Hz and `channels`
    /// channels. Returns null / sets `error != OPUS_OK (0)` on failure.
    fn opus_decoder_create(Fs: opus_int32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    fn opus_decoder_destroy(st: *mut OpusDecoder);
    /// Decodes one Opus packet. `pcm` must hold `frame_size * channels`
    /// interleaved f32 samples. Returns samples-per-channel decoded, or a
    /// negative libopus error code on a bad packet.
    fn opus_decode_float(
        st: *mut OpusDecoder,
        data: *const std::os::raw::c_uchar,
        len: opus_int32,
        pcm: *mut f32,
        frame_size: c_int,
        fec: c_int,
    ) -> c_int;
    fn opus_strerror(error: c_int) -> *const c_char;
}

fn opus_error_message(code: c_int) -> String {
    unsafe {
        let ptr = opus_strerror(code);
        if ptr.is_null() {
            return format!("opus error {code}");
        }
        std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Fields parsed from the OpusHead identification blob (RFC 7845 / dops).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusHead {
    pub channels: u16,
    /// Samples per channel to drop from the very start of the decoded output.
    pub pre_skip: u32,
    /// Original input sample rate (informational; Opus always decodes at 48 kHz).
    pub input_sample_rate: u32,
}

/// Parse the OpusHead side-band. Accepts both container encodings seen in the
/// wild: with the 8-byte `"OpusHead"` magic (WebM/Ogg) and without it, starting
/// at the version byte (the fragmented-MP4 `dops` box).
pub fn parse_opus_head(extra: &[u8]) -> Option<OpusHead> {
    let start = if extra.len() >= 8 && &extra[0..8] == b"OpusHead" {
        8
    } else {
        0
    };
    let h = extra.get(start..)?;
    // layout: version(1) channels(1) pre_skip(2 LE) input_rate(4 LE) gain(2) family(1)
    if h.len() < 11 {
        return None;
    }
    let channels = h[1] as u16;
    if channels == 0 {
        return None;
    }
    let pre_skip = u16::from_le_bytes([h[2], h[3]]) as u32;
    let input_sample_rate = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
    Some(OpusHead {
        channels,
        pre_skip,
        input_sample_rate,
    })
}

/// Stateful decoder for one Opus track: owns the libopus decoder and tracks the
/// remaining pre-skip samples to drop from the start of the stream.
pub struct OpusTrackDecoder {
    dec: *mut OpusDecoder,
    channels: u16,
    pre_skip_remaining: u32,
}

// The raw pointer is only used from the single decode thread; no interior state
// is shared, so Send is sound.
unsafe impl Send for OpusTrackDecoder {}

impl OpusTrackDecoder {
    /// Create a decoder from a parsed OpusHead. Errors on a null decoder or an
    /// unsupported (>2) channel count. Opus always decodes at 48 kHz.
    pub fn new(head: &OpusHead) -> Result<Self, String> {
        if !(1..=2).contains(&head.channels) {
            return Err(format!(
                "Opus channel count {} not supported (only 1-2; multichannel needs the multistream API)",
                head.channels
            ));
        }
        let mut error: c_int = 0;
        let dec = unsafe { opus_decoder_create(48000, head.channels as c_int, &mut error) };
        if dec.is_null() || error != 0 {
            return Err(format!("opus_decoder_create failed: {}", opus_error_message(error)));
        }
        Ok(Self {
            dec,
            channels: head.channels,
            pre_skip_remaining: head.pre_skip,
        })
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Decode one Opus packet into interleaved 48 kHz f32, applying the OpusHead
    /// pre-skip to the first samples. Returns an empty vec for a corrupt packet
    /// (skipped) so a bad packet never aborts playback.
    pub fn push(&mut self, packet: &[u8]) -> Vec<f32> {
        // Largest Opus frame is 120 ms @ 48 kHz = 5760 samples/channel.
        const FRAME: usize = 5760;
        let mut out = vec![0f32; FRAME * self.channels as usize];
        let n = unsafe {
            opus_decode_float(
                self.dec,
                packet.as_ptr(),
                packet.len() as opus_int32,
                out.as_mut_ptr(),
                FRAME as c_int,
                0,
            )
        };
        if n < 0 {
            tracing::warn!("Opus decode of {} byte packet failed: {}", packet.len(), opus_error_message(n));
            return Vec::new();
        }
        let mut pcm = out;
        pcm.truncate(n as usize * self.channels as usize);

        if self.pre_skip_remaining > 0 {
            let drop_samples = (self.pre_skip_remaining as usize * self.channels as usize).min(pcm.len());
            pcm.drain(..drop_samples);
            self.pre_skip_remaining -= (drop_samples / self.channels as usize) as u32;
        }
        pcm
    }
}

impl Drop for OpusTrackDecoder {
    fn drop(&mut self) {
        if !self.dec.is_null() {
            unsafe { opus_decoder_destroy(self.dec) };
            self.dec = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_opus_head_with_magic_webm_ogg() {
        // "OpusHead" | ver=1 | ch=2 | preSkip=312 | rate=48000 | gain=0 | family=0
        let mut b = b"OpusHead".to_vec();
        b.push(1);
        b.push(2);
        b.extend_from_slice(&312u16.to_le_bytes());
        b.extend_from_slice(&48000u32.to_le_bytes());
        b.extend_from_slice(&0i16.to_le_bytes());
        b.push(0);
        let h = parse_opus_head(&b).expect("parse");
        assert_eq!(h.channels, 2);
        assert_eq!(h.pre_skip, 312);
        assert_eq!(h.input_sample_rate, 48000);
    }

    #[test]
    fn parse_opus_head_extended_mono_no_magic_fmp4() {
        // dops-style OpusHead without the magic: ver|ch|preSkip|rate|gain|...
        let mut b = vec![1u8];
        b.push(1); // channels = 1
        b.extend_from_slice(&0u16.to_le_bytes()); // pre_skip
        b.extend_from_slice(&48000u32.to_le_bytes());
        b.extend_from_slice(&0i16.to_le_bytes());
        b.push(0); // family
        let h = parse_opus_head(&b).expect("parse");
        assert_eq!(h.channels, 1);
        assert_eq!(h.pre_skip, 0);
        assert_eq!(h.input_sample_rate, 48000);
    }

    #[test]
    fn parse_opus_head_rejects_garbage_and_zero_channels() {
        assert!(parse_opus_head(b"").is_none());
        assert!(parse_opus_head(b"OpusHe").is_none()); // truncated magic
        let mut b = vec![0u8, 0]; // version=0, channels=0
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        assert!(parse_opus_head(&b).is_none()); // channels == 0
    }

    #[test]
    fn decoder_rejects_multichannel() {
        let head = OpusHead { channels: 6, pre_skip: 0, input_sample_rate: 48000 };
        assert!(OpusTrackDecoder::new(&head).is_err());
    }
}
