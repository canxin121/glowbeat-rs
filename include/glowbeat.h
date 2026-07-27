#ifndef GLOWBEAT_H
#define GLOWBEAT_H

#define GLOWBEAT_ABI_VERSION 1u

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32) && defined(GLOWBEAT_SHARED)
#  if defined(GLOWBEAT_BUILDING_LIBRARY)
#    define GLOWBEAT_API __declspec(dllexport)
#  else
#    define GLOWBEAT_API __declspec(dllimport)
#  endif
#elif defined(__GNUC__) && defined(GLOWBEAT_SHARED)
#  define GLOWBEAT_API __attribute__((visibility("default")))
#else
#  define GLOWBEAT_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct GlowbeatEngineHandle GlowbeatEngineHandle;
typedef struct GlowbeatScoreHandle GlowbeatScoreHandle;
typedef struct GlowbeatPlayerHandle GlowbeatPlayerHandle;

typedef struct GlowbeatFfiConfig {
    uint32_t sample_rate;
    uint32_t channels;
    uint32_t light_capacity;
    uint32_t output_mode; /* 0 = alpha, 1 = physical amber */
    float render_fps;
    float max_brightness;
    float output_gamma;
    float hue_offset;
    float amber_mix;
    float saturation;
    float beat_strength;
    float accent_strength;
    float spatial_strength;
    float max_luma_step;
} GlowbeatFfiConfig;

typedef struct GlowbeatFfiReport {
    uint64_t sample_frames;
    uint64_t analysis_frames;
    uint64_t sample_index;
    uint64_t trailing_samples;
} GlowbeatFfiReport;

typedef struct GlowbeatFfiStatus {
    uint64_t sample_index;
    float loudness;
    float bpm;
    float beat_phase;
    float beat_confidence;
    float buildup;
    float drop;
    uint32_t scene;
} GlowbeatFfiStatus;

typedef struct GlowbeatFfiScoreInfo {
    uint32_t sample_rate;
    uint32_t channels;
    uint32_t analysis_hop;
    uint32_t reserved;
    uint64_t duration_samples;
    uint64_t frame_count;
    uint64_t section_count;
    uint64_t accent_count;
} GlowbeatFfiScoreInfo;

typedef struct GlowbeatLightPoint {
    uint32_t id;
    float x;
    float y;
    float z;
    float zone;
} GlowbeatLightPoint;

typedef struct GlowbeatRgba {
    float r;
    float g;
    float b;
    float a;
} GlowbeatRgba;

/* Return codes: 0 success; -1 invalid argument; -2 capacity mismatch;
 * -4 serialization failure; -5 playback failure; -9 contained Rust panic.
 * Constructors report validation/analysis failure with NULL. */

GLOWBEAT_API uint32_t glowbeat_abi_version(void);
GLOWBEAT_API GlowbeatFfiConfig glowbeat_default_config(void);

/* Causal streaming path. */
GLOWBEAT_API GlowbeatEngineHandle *glowbeat_engine_create(GlowbeatFfiConfig config);
GLOWBEAT_API void glowbeat_engine_destroy(GlowbeatEngineHandle *handle);
GLOWBEAT_API int32_t glowbeat_engine_push_f32(
    GlowbeatEngineHandle *handle,
    const float *samples,
    size_t sample_count,
    GlowbeatFfiReport *report);
GLOWBEAT_API int32_t glowbeat_engine_render(
    GlowbeatEngineHandle *handle,
    uint64_t sample_index,
    const GlowbeatLightPoint *points,
    size_t point_count,
    GlowbeatRgba *output);
GLOWBEAT_API int32_t glowbeat_engine_status(
    const GlowbeatEngineHandle *handle,
    GlowbeatFfiStatus *status);

/* Full-track analysis and portable score cache. */
GLOWBEAT_API GlowbeatScoreHandle *glowbeat_score_analyze_f32(
    const float *samples,
    size_t sample_count,
    uint32_t sample_rate,
    uint32_t channels);
GLOWBEAT_API GlowbeatScoreHandle *glowbeat_score_deserialize(
    const uint8_t *bytes,
    size_t byte_count);
GLOWBEAT_API void glowbeat_score_destroy(GlowbeatScoreHandle *handle);
GLOWBEAT_API int32_t glowbeat_score_info(
    const GlowbeatScoreHandle *handle,
    GlowbeatFfiScoreInfo *info);
GLOWBEAT_API int32_t glowbeat_score_serialized_size(
    const GlowbeatScoreHandle *handle,
    size_t *byte_count);
GLOWBEAT_API int32_t glowbeat_score_serialize(
    const GlowbeatScoreHandle *handle,
    uint8_t *output,
    size_t capacity,
    size_t *written);

/* Sample-clock full-track playback. The player clones the score. */
GLOWBEAT_API GlowbeatPlayerHandle *glowbeat_player_create(
    const GlowbeatScoreHandle *score,
    GlowbeatFfiConfig config);
GLOWBEAT_API void glowbeat_player_destroy(GlowbeatPlayerHandle *handle);
GLOWBEAT_API int32_t glowbeat_player_seek(
    GlowbeatPlayerHandle *handle,
    uint64_t sample_index);
GLOWBEAT_API int32_t glowbeat_player_render(
    GlowbeatPlayerHandle *handle,
    uint64_t sample_index,
    const GlowbeatLightPoint *points,
    size_t point_count,
    GlowbeatRgba *output);

#ifdef __cplusplus
}
#endif

#endif /* GLOWBEAT_H */
