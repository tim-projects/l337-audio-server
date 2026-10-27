/* Link-time stubs for server-side Opus decoding on Android/Termux (G1).
 *
 * These exist ONLY on the CI runner / dev machine at link time. Their SONAME
 * matches the real Termux library, so at runtime the loader resolves DT_NEEDED
 * libopus.so against $PREFIX/lib/libopus.so (supplied by `pkg install libopus`,
 * reached via LD_LIBRARY_PATH=$PREFIX/lib — the same trick as libpulse).
 *
 * The real library's exported symbols are unversioned, so no version script is
 * needed (unlike the libpulse stub). It is an aarch64 bionic object; only the
 * termux variant is a consumer.
 *
 * The implementations are deliberately inert: they must never be executed on
 * device, because the real libopus.so satisfies DT_NEEDED and its symbols bind
 * in its place at load time.
 */

typedef struct OpusEncoder OpusDecoder;

OpusDecoder *opus_decoder_create(int Fs, int channels, int *error) {
    (void)Fs; (void)channels;
    if (error) { *error = -1; }
    return 0;
}

int opus_decode_float(OpusDecoder *st, const unsigned char *data, int len,
                      float *pcm, int frame_size, int fec) {
    (void)st; (void)data; (void)len; (void)pcm; (void)frame_size; (void)fec;
    return -1;
}

void opus_decoder_destroy(OpusDecoder *st) { (void)st; }

const char *opus_strerror(int error) { (void)error; return "opus error"; }
