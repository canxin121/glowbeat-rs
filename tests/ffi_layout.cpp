#include "glowbeat.h"
#include <cstddef>

static_assert(sizeof(GlowbeatRgba) == 16, "GlowbeatRgba ABI changed");
static_assert(sizeof(GlowbeatLightPoint) == 20, "GlowbeatLightPoint ABI changed");
static_assert(sizeof(GlowbeatFfiConfig) == 56, "GlowbeatFfiConfig ABI changed");
static_assert(sizeof(GlowbeatFfiReport) == 32, "GlowbeatFfiReport ABI changed");
static_assert(sizeof(GlowbeatFfiStatus) == 40, "GlowbeatFfiStatus ABI changed");
static_assert(sizeof(GlowbeatFfiScoreInfo) == 48, "GlowbeatFfiScoreInfo ABI changed");

int main() { return 0; }
