/* Port of Pillow's JpegDecode.c/JpegEncode.c calls used by luma/rasterio.py
 * read_bitmap and luma/engine.py export_image. No longjmp crosses Rust. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <limits.h>
#include <setjmp.h>
#include "jpeglib.h"
#include "jerror.h"

#define GRAINY_JPEG_LIMIT ((size_t)512 * 1024 * 1024)
typedef struct {
    unsigned char *data;
    size_t len;
    unsigned int width, height, full_width, full_height, channels;
    char error[256];
} GrainyJpegResult;

typedef struct {
    struct jpeg_error_mgr base;
    jmp_buf jump;
    GrainyJpegResult *result;
} GrainyJpegError;

static void fail(j_common_ptr info) {
    GrainyJpegError *error = (GrainyJpegError *)info->err;
    char text[JMSG_LENGTH_MAX];
    info->err->format_message(info, text);
    snprintf(error->result->error, sizeof(error->result->error), "%s", text);
    longjmp(error->jump, 1);
}
static void quiet(j_common_ptr info) { (void)info; }
static int message(GrainyJpegResult *result, const char *text) {
    snprintf(result->error, sizeof(result->error), "%s", text);
    return 0;
}
static boolean truncated(j_decompress_ptr info) {
    /* jpeg_mem_src otherwise inserts an EOI, unlike Pillow's default strict EOF. */
    ERREXIT(info, JERR_INPUT_EOF);
    return FALSE;
}

int grainy_jpeg_decode(const unsigned char *input, size_t len, size_t target,
                      GrainyJpegResult *result) {
    typedef struct { struct jpeg_decompress_struct info; GrainyJpegError error; } Context;
    Context *ctx;
    size_t stride, count;
    unsigned int scale;
    if (!len || len > ULONG_MAX || len > GRAINY_JPEG_LIMIT)
        return message(result, "JPEG input size outside supported limits");
    ctx = (Context *)calloc(1, sizeof(Context));
    if (!ctx) return message(result, "JPEG allocation failed");
    ctx->info.err = jpeg_std_error(&ctx->error.base);
    ctx->error.base.error_exit = fail;
    ctx->error.base.output_message = quiet;
    ctx->error.result = result;
    if (setjmp(ctx->error.jump)) {
        jpeg_destroy_decompress(&ctx->info);
        free(result->data);
        result->data = NULL;
        result->len = 0;
        free(ctx);
        return 0;
    }
    jpeg_create_decompress(&ctx->info);
    jpeg_mem_src(&ctx->info, input, (unsigned long)len);
    ctx->info.src->fill_input_buffer = truncated;
    jpeg_read_header(&ctx->info, TRUE);
    if (ctx->info.data_precision != 8) ERREXIT1(&ctx->info, JERR_BAD_PRECISION, ctx->info.data_precision);
    result->full_width = ctx->info.image_width;
    result->full_height = ctx->info.image_height;
    switch (ctx->info.num_components) {
        case 1: ctx->info.out_color_space = JCS_GRAYSCALE; break;
        case 3: ctx->info.out_color_space = JCS_RGB; break;
        case 4: ctx->info.out_color_space = JCS_CMYK; break;
        default: ERREXIT(&ctx->info, JERR_BAD_J_COLORSPACE);
    }
    scale = 1;
    if (target) {
        for (scale = 8; scale > 1; scale /= 2) {
            if (ctx->info.image_width / scale >= target && ctx->info.image_height / scale >= target) break;
        }
    }
    ctx->info.scale_num = 1;
    ctx->info.scale_denom = scale;
    ctx->info.dct_method = JDCT_ISLOW;
    ctx->info.do_fancy_upsampling = TRUE;
    jpeg_calc_output_dimensions(&ctx->info);
    stride = (size_t)ctx->info.output_width * ctx->info.out_color_components;
    if (!stride || !ctx->info.output_height || stride > GRAINY_JPEG_LIMIT / ctx->info.output_height)
        ERREXIT1(&ctx->info, JERR_OUT_OF_MEMORY, 0);
    count = stride * ctx->info.output_height;
    result->data = (unsigned char *)malloc(count);
    if (!result->data) ERREXIT1(&ctx->info, JERR_OUT_OF_MEMORY, 0);
    jpeg_start_decompress(&ctx->info);
    result->width = ctx->info.output_width;
    result->height = ctx->info.output_height;
    result->channels = ctx->info.output_components;
    while (ctx->info.output_scanline < ctx->info.output_height) {
        JSAMPROW row = result->data + (size_t)ctx->info.output_scanline * stride;
        jpeg_read_scanlines(&ctx->info, &row, 1);
    }
    jpeg_finish_decompress(&ctx->info);
    jpeg_destroy_decompress(&ctx->info);
    free(ctx);
    result->len = count;
    /* JpegImagePlugin always requests CMYK;I, even with no Adobe APP14 marker. */
    if (result->channels == 4) {
        size_t i;
        for (i = 0; i < count; ++i) result->data[i] = 255 - result->data[i];
    }
    return 1;
}

int grainy_jpeg_encode(const unsigned char *pixels, unsigned int width, unsigned int height,
                      unsigned int channels, int quality, const unsigned char *icc, size_t icc_len,
                      const unsigned char *exif, size_t exif_len, GrainyJpegResult *result) {
    /* Destination ownership stays on the heap across setjmp, including errors during finish. */
    typedef struct {
        struct jpeg_compress_struct info;
        GrainyJpegError error;
        unsigned char *output;
        unsigned long output_len;
    } Context;
    Context *ctx = (Context *)calloc(1, sizeof(Context));
    unsigned int i;
    size_t stride = (size_t)width * channels;
    if (!ctx) return message(result, "JPEG allocation failed");
    ctx->info.err = jpeg_std_error(&ctx->error.base);
    ctx->error.base.error_exit = fail;
    ctx->error.base.output_message = quiet;
    ctx->error.result = result;
    if (setjmp(ctx->error.jump)) {
        jpeg_destroy_compress(&ctx->info);
        free(ctx->output);
        free(ctx);
        return 0;
    }
    jpeg_create_compress(&ctx->info);
    jpeg_mem_dest(&ctx->info, &ctx->output, &ctx->output_len);
    ctx->info.image_width = width;
    ctx->info.image_height = height;
    ctx->info.input_components = channels;
    ctx->info.in_color_space = channels == 1 ? JCS_GRAYSCALE : JCS_RGB;
    jpeg_set_defaults(&ctx->info);
    if (quality != -1) jpeg_set_quality(&ctx->info, quality, TRUE);
    for (i = 0; i < (unsigned int)ctx->info.num_components; ++i) {
        ctx->info.comp_info[i].h_samp_factor = 1;
        ctx->info.comp_info[i].v_samp_factor = 1;
    }
    ctx->info.dct_method = JDCT_ISLOW;
    ctx->info.optimize_coding = FALSE;
    ctx->info.smoothing_factor = 0;
    jpeg_start_compress(&ctx->info, TRUE);
    /* Pillow: SOI, JFIF, EXIF, extra (ICC), then tables and entropy-coded scan. */
    if (exif_len) jpeg_write_marker(&ctx->info, JPEG_APP0 + 1, exif, (unsigned int)exif_len);
    if (icc_len) jpeg_write_icc_profile(&ctx->info, icc, (unsigned int)icc_len);
    while (ctx->info.next_scanline < ctx->info.image_height) {
        JSAMPROW row = (JSAMPROW)(pixels + (size_t)ctx->info.next_scanline * stride);
        jpeg_write_scanlines(&ctx->info, &row, 1);
    }
    jpeg_finish_compress(&ctx->info);
    jpeg_destroy_compress(&ctx->info);
    result->data = ctx->output;
    result->len = ctx->output_len;
    free(ctx);
    return 1;
}

void grainy_jpeg_free(unsigned char *data) { free(data); }
