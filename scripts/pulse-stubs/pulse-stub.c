/* Link-time stubs for the Android/Termux PulseAudio backend.
 *
 * PulseAudio's client library only initializes correctly when the main
 * executable is linked against it at BUILD time: a dlopen'd libpulse fails
 * client-side in pa_stream_new (PA_ERR_INVALID) on bionic, while the same
 * calls in a binary linked against the real library succeed (see
 * plan-android.md sections 4.6 and 4.8 for the on-device bisect).
 *
 * These stubs exist ONLY on the CI runner / dev machine at link time. Their
 * SONAMEs and symbol versions match the real Termux libraries, so at runtime
 * the loader resolves the DT_NEEDED entries against $PREFIX/lib/libpulse*.so.
 * They are aarch64 bionic objects; the termux variant is the only consumer.
 */
typedef void pa_simple;
typedef unsigned long pa_usec_t;

pa_simple *pa_simple_new(const char *server, const char *name, int dir,
                         const char *dev, const char *stream_name,
                         const void *ss, const void *map, const void *attr,
                         int *error) {
    (void)server; (void)name; (void)dir; (void)dev; (void)stream_name;
    (void)ss; (void)map; (void)attr; (void)error;
    return 0;
}
int pa_simple_write(pa_simple *s, const void *data, unsigned long n, int *error) {
    (void)s; (void)data; (void)n; (void)error;
    return -1;
}
int pa_simple_drain(pa_simple *s, int *error) {
    (void)s; (void)error;
    return -1;
}
void pa_simple_free(pa_simple *s) { (void)s; }
pa_usec_t pa_simple_get_latency(pa_simple *s, int *error) {
    (void)s; (void)error;
    return 0;
}
