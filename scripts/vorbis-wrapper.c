#include <vorbis/vorbisfile.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    OggVorbis_File vf;
    vorbis_info* vi;
    int channels;
    int sample_rate;
    double duration;
    long total_samples;
    int bitstream;
    short buffer[4096 * 2];
    int initialized;
} VorbisDecoder;

VorbisDecoder* vorbis_decoder_open(const char* path) {
    VorbisDecoder* dec = calloc(1, sizeof(VorbisDecoder));
    if (!dec) return NULL;
    
    int ret = ov_fopen(path, &dec->vf);
    if (ret < 0) {
        free(dec);
        return NULL;
    }
    
    dec->vi = ov_info(&dec->vf, -1);
    if (!dec->vi) {
        ov_clear(&dec->vf);
        free(dec);
        return NULL;
    }
    
    dec->channels = dec->vi->channels;
    dec->sample_rate = dec->vi->rate;
    dec->duration = ov_time_total(&dec->vf, -1);
    dec->total_samples = ov_pcm_total(&dec->vf, -1);
    dec->initialized = 1;
    
    return dec;
}

int vorbis_decoder_get_info(VorbisDecoder* dec, int* out_channels, int* out_sample_rate, double* out_duration, long* out_total_samples) {
    if (!dec || !dec->initialized) return -1;
    if (out_channels) *out_channels = dec->channels;
    if (out_sample_rate) *out_sample_rate = dec->sample_rate;
    if (out_duration) *out_duration = dec->duration;
    if (out_total_samples) *out_total_samples = dec->total_samples;
    return 0;
}

int vorbis_decoder_read_chunk(VorbisDecoder* dec, float* out_buffer, int max_samples, int* out_samples_read) {
    if (!dec || !dec->initialized) return -1;
    
    int max_bytes = max_samples * dec->channels * 2; // 16-bit
    if (max_bytes > (int)sizeof(dec->buffer)) {
        max_bytes = sizeof(dec->buffer);
    }
    
    long ret = ov_read(&dec->vf, (char*)dec->buffer, max_bytes, 0, 2, 1, &dec->bitstream);
    if (ret <= 0) {
        *out_samples_read = 0;
        return ret < 0 ? -1 : 0; // 0 = EOF, -1 = error
    }
    
    int samples = ret / 2; // 16-bit = 2 bytes per sample
    int frames = samples / dec->channels;
    
    for (int i = 0; i < samples; i++) {
        out_buffer[i] = dec->buffer[i] / 32768.0f; // i16 to f32 [-1, 1]
    }
    
    *out_samples_read = samples;
    return 1; // more data available
}

double vorbis_decoder_get_time(VorbisDecoder* dec) {
    if (!dec || !dec->initialized) return -1.0;
    return ov_time_tell(&dec->vf);
}

long vorbis_decoder_get_pcm_pos(VorbisDecoder* dec) {
    if (!dec || !dec->initialized) return -1;
    return ov_pcm_tell(&dec->vf);
}

int vorbis_decoder_seek_pcm(VorbisDecoder* dec, long pcm_pos) {
    if (!dec || !dec->initialized) return -1;
    return ov_pcm_seek(&dec->vf, pcm_pos);
}

int vorbis_decoder_seek_time(VorbisDecoder* dec, double time_sec) {
    if (!dec || !dec->initialized) return -1;
    return ov_time_seek(&dec->vf, time_sec);
}

void vorbis_decoder_close(VorbisDecoder* dec) {
    if (dec) {
        if (dec->initialized) {
            ov_clear(&dec->vf);
        }
        free(dec);
    }
}

// Legacy one-shot API for compatibility
int decode_vorbis_file_c(const char* path, float** out_data, int* out_len, int* out_channels, int* out_sample_rate, double* out_duration) {
    VorbisDecoder* dec = vorbis_decoder_open(path);
    if (!dec) return -1;
    
    int channels, sample_rate;
    double duration;
    long total_samples;
    vorbis_decoder_get_info(dec, &channels, &sample_rate, &duration, &total_samples);
    
    if (total_samples <= 0) {
        vorbis_decoder_close(dec);
        return -1;
    }
    
    float* data = malloc(total_samples * channels * sizeof(float));
    if (!data) {
        vorbis_decoder_close(dec);
        return -1;
    }
    
    float* dst = data;
    int samples_read;
    while (1) {
        int ret = vorbis_decoder_read_chunk(dec, dst, 4096 * dec->channels, &samples_read);
        if (ret <= 0) break;
        dst += samples_read;
    }
    
    vorbis_decoder_close(dec);
    
    *out_data = data;
    *out_len = (int)(dst - data);
    *out_channels = channels;
    *out_sample_rate = sample_rate;
    *out_duration = duration;
    return 0;
}

void free_vorbis_result(float* data) {
    free(data);
}