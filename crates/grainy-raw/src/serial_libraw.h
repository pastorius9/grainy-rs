// Keep LibRaw's inline allocator identical and synchronized in all translation
// units. Only the implementation-file regions are serial, especially X-Trans.
// This forced include changes no vendored source bytes.
#define LIBRAW_LIBRARY_BUILD
#include "libraw/libraw.h"
#undef LIBRAW_USE_OPENMP
