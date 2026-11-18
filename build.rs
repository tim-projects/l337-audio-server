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
/// On Termux/Android with the `vorbis` feature, link against Termux's native
/// libraries which provide NEON-optimized Vorbis decoding. This replaces
/// Symphonia's slow pure-Rust Vorbis decoder.
///
/// On Linux, we expect the system libvorbisfile/vorbis/ogg to be available
/// via pkg-config or standard library paths. The C wrapper (`scripts/vorbis-wrapper.c`)
/// is compiled to `libl337vorbis.so` by the build script or CI.
///
/// If native libraries are not available, the build still succeeds and the
/// Symphonia pure-Rust Vorbis decoder is used as fallback (see src/player/vorbis.rs).
fn link_vorbis() {
    if env::var_os("CARGO_FEATURE_VORBIS").is_none() {
        return;
    }

    let target_os = env::var_os("CARGO_CFG_TARGET_OS");
    let os = target_os.as_deref();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let wrapper_lib = format!("{}/scripts/vorbis-wrapper", manifest_dir);

    if os == Some(OsStr::new("android")) {
        // Termux/Android: link against Termux's native libraries
        let termux_lib = "/data/data/com.termux/files/usr/lib";
        println!("cargo:rustc-link-search=native={}", termux_lib);
        println!("cargo:rustc-link-lib=dylib=l337vorbis");
        println!("cargo:rerun-if-changed={}/vorbis-wrapper.c", manifest_dir);
    } else if os == Some(OsStr::new("linux")) {
        // Linux: try to link system libvorbisfile/vorbis/ogg
        // The C wrapper must be compiled separately (by CI or build script)
        if Path::new(&format!("{}/target/release/libl337vorbis.so", manifest_dir)).exists()
            || Path::new(&format!("{}/target/debug/libl337vorbis.so", manifest_dir)).exists()
            || Path::new("/usr/lib/libl337vorbis.so").exists()
            || Path::new("/usr/local/lib/libl337vorbis.so").exists()
        {
            println!("cargo:rustc-link-lib=dylib=l337vorbis");
            println!("cargo:rerun-if-changed={}/vorbis-wrapper.c", manifest_dir);
        } else {
            eprintln!("cargo:warning=libl337vorbis.so not found; Vorbis will fall back to Symphonia pure-Rust decoder");
        }
    }
}
