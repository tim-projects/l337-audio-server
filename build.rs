use std::env;
use std::ffi::OsStr;
use std::path::Path;

fn main() {
    let version = env::var("L337_VERSION").unwrap_or_else(|_| "0.0.0-dev".to_string());
    println!("cargo:rustc-env=L337_VERSION={}", version);

    link_pulseaudio_for_termux();
    link_opus();
    link_vorbis();
}

/// Link the real libopus for server-side Opus decoding (see src/player/opus.rs).
///
/// Gated on the `opus` crate feature so cross targets that cannot supply libopus
/// never link it, and on `linux`/`android`:
///   - linux: link the system libopus (libopus-dev on CI; `pkg install libopus`
///     on Termux-native).
///   - android: the NDK has no system opus, so link the stub in
///     `scripts/opus-stubs` (SONAME libopus.so), which the loader resolves
///     against the real `$PREFIX/lib/libopus.so` at runtime — the same strategy
///     as the libpulse stub, and never `dlopen`.
fn link_opus() {
    if env::var_os("CARGO_FEATURE_OPUS").is_none() {
        return;
    }
    let target_os = env::var_os("CARGO_CFG_TARGET_OS");
    let os = target_os.as_deref();
    if os == Some(OsStr::new("linux")) {
        println!("cargo:rustc-link-lib=dylib=opus");
    } else if os == Some(OsStr::new("android")) {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let stubs = format!("{}/scripts/opus-stubs", manifest_dir);
        println!("cargo:rustc-link-search=native={}", stubs);
        println!("cargo:rustc-link-lib=dylib=opus");
        println!("cargo:rerun-if-changed={}", stubs);
    }
}

/// Link the Android/Termux PulseAudio backend against libpulse at build time.
///
/// PulseAudio's client library only initializes correctly when the main
/// executable is linked against it at build time: a dlopen'd libpulse fails
/// client-side in `pa_stream_new` (`PA_ERR_INVALID`) on bionic, while the same
/// calls in a binary linked against the real library succeed. The on-device
/// bisect that established this is recorded in `plan-android.md` (sections 4.6
/// and 4.8).
///
/// The linked stub in `scripts/pulse-stubs` exists only at link time; its
/// SONAME and symbol versions match the real Termux library, so at runtime the
/// loader resolves the `DT_NEEDED` entry against `$PREFIX/lib/libpulse.so`.
///
/// We link only `libpulse.so` — never `libpulse-simple.so` — because the
/// Termux build's `pa_simple_new` fails in this process while the equivalent
/// documented `pa_threaded_mainloop`/`pa_context`/`pa_stream` flow works
/// (plan-android.md section 4.9).
fn link_pulseaudio_for_termux() {
    let target_os = env::var_os("CARGO_CFG_TARGET_OS");
    if target_os.as_deref() != Some(OsStr::new("android")) {
        return;
    }
    // `pulseaudio` also enables `backend`, which selects the Android backend.
    if env::var_os("CARGO_FEATURE_PULSEAUDIO").is_none() {
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let stubs = format!("{}/scripts/pulse-stubs", manifest_dir);
    println!("cargo:rustc-link-search=native={}", stubs);
    println!("cargo:rustc-link-lib=dylib=pulse");
    println!("cargo:rerun-if-changed={}", stubs);
}

/// Link native libvorbis/vorbisfile/ogg for fast Vorbis decoding.
///
/// Strategy by platform:
/// - **Linux (CI & native):** Use `cc` crate to compile `scripts/vorbis-wrapper.c`
///   as a **static library** and link it directly into the binary. This embeds
///   the Vorbis decode logic + links system libvorbisfile/vorbis/ogg statically
///   where possible. No separate `libl337vorbis.so` needed.
/// - **Termux/Android:** Link dynamically against Termux's native libraries
///   (`libvorbisfile.so`, `libvorbis.so`, `libogg.so`) via the pre-built
///   `libl337vorbis.so` at `/data/data/com.termux/files/usr/lib`. The wrapper
///   must be compiled on the device (or cross-compiled for CI — see below).
/// - **Windows/macOS/other:** No native Vorbis; falls back to Symphonia pure-Rust
///   decoder automatically (see src/player/vorbis.rs).
///
/// CI note: Linux jobs must install `libvorbis-dev libogg-dev pkg-config` before
/// building. The `cc` crate will invoke the C compiler to build the static lib.
fn link_vorbis() {
    if env::var_os("CARGO_FEATURE_VORBIS").is_none() {
        return;
    }

    let target_os = env::var_os("CARGO_CFG_TARGET_OS");
    let os = target_os.as_deref();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");

    if os == Some(OsStr::new("linux")) {
        // Linux: compile the C wrapper statically and link it into the binary.
        // This requires libvorbis-dev / libogg-dev on the build machine.
        // The cc crate handles invoking the C compiler with correct flags.
        let mut cfg = cc::Build::new();
        cfg.file(format!("{}/scripts/vorbis-wrapper.c", manifest_dir))
            .include(&manifest_dir)
            .flag("-O3")
            .flag("-fPIC")
            .define("VORBIS_STATIC", None)  // hint for static linking if supported
            .compile("l337vorbis"); // produces libl337vorbis.a linked into rlib

        // Link system libvorbisfile, libvorbis, libogg (dynamic by default).
        // On the pinned Debian bookworm CI image these are available.
        println!("cargo:rustc-link-lib=vorbisfile");
        println!("cargo:rustc-link-lib=vorbis");
        println!("cargo:rustc-link-lib=ogg");
        println!("cargo:rerun-if-changed={}/scripts/vorbis-wrapper.c", manifest_dir);

    } else if os == Some(OsStr::new("android")) {
        // Termux/Android: link against Termux's native libraries.
        // The C wrapper is pre-compiled to libl337vorbis.so on the device.
        // CI cannot easily cross-compile libvorbis for Android/Termux,
        // so the termux binary expects the .so at runtime via LD_LIBRARY_PATH.
        let termux_lib = "/data/data/com.termux/files/usr/lib";
        println!("cargo:rustc-link-search=native={}", termux_lib);
        println!("cargo:rustc-link-lib=dylib=l337vorbis");
        println!("cargo:rerun-if-changed={}/scripts/vorbis-wrapper.c", manifest_dir);

    } else {
        // Windows, macOS, etc.: no native Vorbis support in CI.
        // The Rust code falls back to Symphonia's pure-Rust Vorbis decoder.
        eprintln!("cargo:warning=Native Vorbis not available on this platform; falling back to Symphonia pure-Rust decoder");
    }
}