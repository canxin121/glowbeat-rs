#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include "glowbeat.h"

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(sizeof(GlowbeatFfiConfig) == 56, "GlowbeatFfiConfig ABI mismatch");
_Static_assert(sizeof(GlowbeatFfiReport) == 32, "GlowbeatFfiReport ABI mismatch");
_Static_assert(sizeof(GlowbeatFfiStatus) == 40, "GlowbeatFfiStatus ABI mismatch");
_Static_assert(offsetof(GlowbeatFfiStatus, scene) == 32, "GlowbeatFfiStatus ABI mismatch");
_Static_assert(sizeof(GlowbeatFfiScoreInfo) == 48, "GlowbeatFfiScoreInfo ABI mismatch");
_Static_assert(sizeof(GlowbeatLightPoint) == 20, "GlowbeatLightPoint ABI mismatch");
_Static_assert(sizeof(GlowbeatRgba) == 16, "GlowbeatRgba ABI mismatch");
#endif

int main(void) {
    if (glowbeat_abi_version() != GLOWBEAT_ABI_VERSION) {
        fputs("GlowBeat ABI version mismatch\n", stderr);
        return 2;
    }

    GlowbeatFfiConfig config = glowbeat_default_config();
    config.sample_rate = 48000;
    config.channels = 2;
    config.light_capacity = 4;

    GlowbeatEngineHandle *engine = glowbeat_engine_create(config);
    if (engine == NULL) {
        fputs("failed to create GlowBeat engine\n", stderr);
        return 3;
    }

    float silence[512 * 2] = {0};
    GlowbeatFfiReport report = {0};
    if (glowbeat_engine_push_f32(engine, silence, 512 * 2, &report) != 0) {
        glowbeat_engine_destroy(engine);
        return 4;
    }

    GlowbeatLightPoint points[4] = {
        {0, -1.0f, 0.0f, 0.0f, 0.0f},
        {1, -0.33f, 0.0f, 0.0f, 0.33f},
        {2,  0.33f, 0.0f, 0.0f, 0.66f},
        {3,  1.0f, 0.0f, 0.0f, 1.0f},
    };
    GlowbeatRgba rgba[4] = {0};
    if (glowbeat_engine_render(engine, report.sample_index, points, 4, rgba) != 0) {
        glowbeat_engine_destroy(engine);
        return 5;
    }

    printf("sample=%llu first=(%.3f, %.3f, %.3f, %.3f)\n",
           (unsigned long long)report.sample_index,
           rgba[0].r, rgba[0].g, rgba[0].b, rgba[0].a);
    glowbeat_engine_destroy(engine);
    return 0;
}
