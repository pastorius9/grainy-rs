/* Selected by build.rs for the 8-bit Windows x64 compressor. The decoder
 * continues to call the scalar functions from jsimd_none.c. */
#include "native_symbols.h"
#define WITH_SIMD 1

#undef jsimd_can_rgb_ycc
#undef jsimd_can_convsamp
#undef jsimd_can_fdct_islow
#undef jsimd_can_quantize
#undef jsimd_can_huff_encode_one_block
#define jsimd_can_rgb_ycc grainy_jpeg_encoder_has_sse2
#define jsimd_can_convsamp grainy_jpeg_encoder_has_sse2
#define jsimd_can_fdct_islow grainy_jpeg_encoder_has_sse2
#define jsimd_can_quantize grainy_jpeg_encoder_has_sse2
#define jsimd_can_huff_encode_one_block grainy_jpeg_encoder_has_sse2

#undef jsimd_rgb_ycc_convert
#undef jsimd_convsamp
#undef jsimd_fdct_islow
#undef jsimd_quantize
#undef jsimd_huff_encode_one_block
#define jsimd_rgb_ycc_convert grainy_jpeg_encoder_rgb_ycc
#define jsimd_convsamp grainy_jpeg_jsimd_convsamp_sse2
#define jsimd_fdct_islow grainy_jpeg_jsimd_fdct_islow_sse2
#define jsimd_quantize grainy_jpeg_jsimd_quantize_sse2
#define jsimd_huff_encode_one_block grainy_jpeg_jsimd_huff_encode_one_block_sse2
