// Keep LibRaw's inline allocator identical and synchronized in all translation
// units. Suppress upstream OpenMP regions in this group. Grainy's MSVC X-Trans
// derivative schedules independent tiles using its own OpenMP work-sharing loop.
// This forced include changes no vendored source bytes.
#define LIBRAW_LIBRARY_BUILD
#include "libraw/libraw.h"
#undef LIBRAW_USE_OPENMP
