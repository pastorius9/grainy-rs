#define JPEG_INTERNALS
#include "jinclude.h"
#include "jpeglib.h"
#include "jdct.h"

/* These are the sample sizes and RGB layout in the assembly configuration. */
typedef char grainy_jpeg_check_sse2_abi[
    (DCTSIZE == 8 && sizeof(JDIMENSION) == 4 && sizeof(JSAMPLE) == 1 &&
     sizeof(DCTELEM) == 2 && sizeof(JCOEF) == 2 && RGB_PIXELSIZE == 3 &&
     RGB_RED == 0 && RGB_GREEN == 1 && RGB_BLUE == 2) ? 1 : -1];

int grainy_jpeg_encoder_has_sse2(void) { return 1; }  /* Required on x86-64. */

typedef void rgb_to_ycc(JDIMENSION, JSAMPARRAY, JSAMPIMAGE, JDIMENSION, int);
extern rgb_to_ycc grainy_jpeg_jsimd_rgb_ycc_convert_sse2;
extern rgb_to_ycc grainy_jpeg_jsimd_extbgr_ycc_convert_sse2;
extern rgb_to_ycc grainy_jpeg_jsimd_extrgbx_ycc_convert_sse2;
extern rgb_to_ycc grainy_jpeg_jsimd_extbgrx_ycc_convert_sse2;
extern rgb_to_ycc grainy_jpeg_jsimd_extxbgr_ycc_convert_sse2;
extern rgb_to_ycc grainy_jpeg_jsimd_extxrgb_ycc_convert_sse2;

void grainy_jpeg_encoder_rgb_ycc(j_compress_ptr cinfo, JSAMPARRAY input,
                                 JSAMPIMAGE output, JDIMENSION row, int rows) {
    rgb_to_ycc *convert;
    switch (cinfo->in_color_space) {
    case JCS_EXT_BGR: convert = grainy_jpeg_jsimd_extbgr_ycc_convert_sse2; break;
    case JCS_EXT_RGBX: case JCS_EXT_RGBA: convert = grainy_jpeg_jsimd_extrgbx_ycc_convert_sse2; break;
    case JCS_EXT_BGRX: case JCS_EXT_BGRA: convert = grainy_jpeg_jsimd_extbgrx_ycc_convert_sse2; break;
    case JCS_EXT_XBGR: case JCS_EXT_ABGR: convert = grainy_jpeg_jsimd_extxbgr_ycc_convert_sse2; break;
    case JCS_EXT_XRGB: case JCS_EXT_ARGB: convert = grainy_jpeg_jsimd_extxrgb_ycc_convert_sse2; break;
    default: convert = grainy_jpeg_jsimd_rgb_ycc_convert_sse2; break;
    }
    convert(cinfo->image_width, input, output, row, rows);
}
