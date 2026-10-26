/* Link-time stubs for the Android/Termux PulseAudio backend.
 *
 * Two findings drive this (see plan-android.md sections 4.6, 4.8 and 4.9):
 *
 *  1. PulseAudio's client library only initializes correctly when the main
 *     executable is linked against it at BUILD time. A dlopen'd libpulse fails
 *     client-side (PA_ERR_INVALID), while the identical calls in a linked
 *     binary succeed - even when the caller is a large PIE Rust executable.
 *
 *  2. The Termux build of libpulse-simple.so (17.0-dirty) has a patched
 *     pa_simple_new that returns PA_ERR_INVALID inside our Rust process,
 *     although re-implementing its documented flow with the libpulse.so public
 *     API succeeds in the very same process (context READY, stream READY,
 *     "Using PulseAudio backend" logged). We therefore drive the
 *     pa_threaded_mainloop / pa_context / pa_stream API directly and never
 *     call pa_simple_new.
 *
 * These stubs exist ONLY on the CI runner / dev machine at link time. Their
 * SONAME and symbol versions match the real Termux library, so at runtime the
 * loader resolves DT_NEEDED libpulse.so against $PREFIX/lib/libpulse.so.
 * They are aarch64 bionic objects; the termux variant is the only consumer.
 */

typedef void pa_threaded_mainloop;
typedef void pa_context;
typedef void pa_stream;
typedef unsigned long long pa_usec_t;

/* --- threaded mainloop --- */
pa_threaded_mainloop *pa_threaded_mainloop_new(void) { return 0; }
void pa_threaded_mainloop_free(pa_threaded_mainloop *m) { (void)m; }
void *pa_threaded_mainloop_get_api(pa_threaded_mainloop *m) { (void)m; return 0; }
int pa_threaded_mainloop_start(pa_threaded_mainloop *m) { (void)m; return -1; }
void pa_threaded_mainloop_stop(pa_threaded_mainloop *m) { (void)m; }
int pa_threaded_mainloop_lock(pa_threaded_mainloop *m) { (void)m; return -1; }
int pa_threaded_mainloop_unlock(pa_threaded_mainloop *m) { (void)m; return -1; }
void pa_threaded_mainloop_signal(pa_threaded_mainloop *m, int wait_for_accept) {
    (void)m; (void)wait_for_accept;
}
int pa_threaded_mainloop_wait(pa_threaded_mainloop *m) { (void)m; return -1; }

/* --- context --- */
pa_context *pa_context_new(void *mainloop_api, const char *name) {
    (void)mainloop_api; (void)name;
    return 0;
}
pa_context *pa_context_ref(pa_context *c) { (void)c; return 0; }
void pa_context_unref(pa_context *c) { (void)c; }
typedef void (*pa_context_notify_cb_t)(pa_context *c, void *userdata);
void pa_context_set_state_callback(pa_context *c, pa_context_notify_cb_t cb, void *userdata) {
    (void)c; (void)cb; (void)userdata;
}
int pa_context_connect(pa_context *c, const char *server, int flags, const void *api) {
    (void)c; (void)server; (void)flags; (void)api;
    return -1;
}
void pa_context_disconnect(pa_context *c) { (void)c; }
int pa_context_get_state(pa_context *c) { (void)c; return 5; }
int pa_context_errno(pa_context *c) { (void)c; return 3; }

/* --- stream --- */
typedef struct pa_sample_spec { int format; unsigned rate; unsigned char channels; } pa_sample_spec;
pa_stream *pa_stream_new(pa_context *c, const char *name, const pa_sample_spec *ss, const void *map) {
    (void)c; (void)name; (void)ss; (void)map;
    return 0;
}
typedef void (*pa_stream_notify_cb_t)(pa_stream *p, void *userdata);
void pa_stream_set_state_callback(pa_stream *s, pa_stream_notify_cb_t cb, void *userdata) {
    (void)s; (void)cb; (void)userdata;
}
int pa_stream_connect_playback(pa_stream *s, const char *dev, const void *attr,
                               unsigned int flags, const void *volume, pa_stream *sync_stream) {
    (void)s; (void)dev; (void)attr; (void)flags; (void)volume; (void)sync_stream;
    return -1;
}
typedef int pa_seek_mode_t;
int pa_stream_write(pa_stream *s, const void *data, unsigned long nbytes, void *free_cb, long long offset, pa_seek_mode_t seek) {
    (void)s; (void)data; (void)nbytes; (void)free_cb; (void)offset; (void)seek;
    return -1;
}
typedef void (*pa_stream_success_cb_t)(pa_stream *s, int success, void *userdata);
void *pa_stream_drain(pa_stream *s, pa_stream_success_cb_t cb, void *userdata) {
    (void)s; (void)cb; (void)userdata;
    return 0;
}
int pa_stream_disconnect(pa_stream *s) { (void)s; return -1; }
int pa_stream_get_state(pa_stream *s) { (void)s; return 5; }
void pa_stream_unref(pa_stream *s) { (void)s; }

/* --- errors --- */
const char *pa_strerror(int error) { (void)error; return "PulseAudio error"; }
