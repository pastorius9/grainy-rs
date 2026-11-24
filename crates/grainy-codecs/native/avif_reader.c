#include <avif/avif.h>
#include <avif/internal.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    uint32_t width, height, channels, depth, orientation, primaries, transfer, alpha_premultiplied;
    const uint16_t *pixels;
    const uint8_t *icc;
    size_t icc_size;
    avifDecoder *decoder;
    avifRGBImage rgb;
} GrainyAvif;

// libavif invokes this only when a layered image requires resizing a decoded tile.
// This decoder has no libyuv dependency; never silently substitute an incorrectly sized tile.
avifResult avifImageScaleWithLimit(avifImage *image, uint32_t w, uint32_t h, uint32_t sizeLimit,
                                   uint32_t dimensionLimit, avifDiagnostics *diag) {
    (void)sizeLimit; (void)dimensionLimit; (void)diag;
    return image->width == w && image->height == h ? AVIF_RESULT_OK : AVIF_RESULT_NOT_IMPLEMENTED;
}

avifResult avifImageScale(avifImage *image, uint32_t w, uint32_t h, avifDiagnostics *diag) {
    return avifImageScaleWithLimit(image, w, h, AVIF_DEFAULT_IMAGE_SIZE_LIMIT, AVIF_DEFAULT_IMAGE_DIMENSION_LIMIT, diag);
}

void grainy_avif_close(GrainyAvif *r) {
    if (!r) return;
    avifRGBImageFreePixels(&r->rgb);
    avifDecoderDestroy(r->decoder);
    free(r);
}

// Deliver only the primary image's TIFF payload while the decoder owns it.
void grainy_avif_exif(const uint8_t *data, size_t len,
                      void (*receive)(const uint8_t *, size_t, void *), void *context) {
    avifROData input = { data, len };
    if (!avifPeekCompatibleFileType(&input)) return;
    avifDecoder *decoder = avifDecoderCreate();
    if (!decoder) return;
    decoder->ignoreXMP = AVIF_TRUE;
    if (avifDecoderSetIOMemory(decoder, data, len) == AVIF_RESULT_OK &&
        avifDecoderParse(decoder) == AVIF_RESULT_OK) {
        const avifRWData *exif = &decoder->image->exif;
        size_t offset;
        if (exif->size && exif->size <= 16 * 1024 * 1024 &&
            avifGetExifTiffHeaderOffset(exif->data, exif->size, &offset) == AVIF_RESULT_OK) {
            receive(exif->data + offset, exif->size - offset, context);
        }
    }
    avifDecoderDestroy(decoder);
}

const char *grainy_avif_read(const uint8_t *data, size_t len, GrainyAvif **out) {
    GrainyAvif *r = calloc(1, sizeof(*r));
    if (!r) return "AVIF allocation failed";
    r->decoder = avifDecoderCreate();
    if (!r->decoder) { free(r); return "AVIF decoder allocation failed"; }
    avifDecoder *d = r->decoder;
    d->maxThreads = 2;
    d->imageSizeLimit = 128U * 1024U * 1024U;
    d->ignoreXMP = AVIF_TRUE;
    avifResult status = avifDecoderSetIOMemory(d, data, len);
    if (status == AVIF_RESULT_OK) status = avifDecoderParse(d);
    if (status == AVIF_RESULT_OK) status = avifDecoderNextImage(d);
    if (status != AVIF_RESULT_OK) goto failed;
    const avifImage *im = d->image;
    avifImage view;
    if (im->transformFlags & AVIF_TRANSFORM_CLAP) {
        avifCropRect crop;
        if (!avifCropRectFromCleanApertureBox(&crop, &im->clap, im->width, im->height, &d->diag)) {
            status = AVIF_RESULT_INVALID_ARGUMENT; goto failed;
        }
        memset(&view, 0, sizeof(view));
        status = avifImageSetViewRect(&view, im, &crop);
        if (status != AVIF_RESULT_OK) goto failed;
        im = &view;
    }
    avifRGBImageSetDefaults(&r->rgb, im);
    r->rgb.alphaPremultiplied = AVIF_FALSE;
    r->rgb.depth = 16;
    r->rgb.format = im->alphaPlane ? AVIF_RGB_FORMAT_RGBA : AVIF_RGB_FORMAT_RGB;
    status = avifRGBImageAllocatePixels(&r->rgb);
    if (status != AVIF_RESULT_OK) goto failed;
    status = avifImageYUVToRGB(im, &r->rgb);
    if (status != AVIF_RESULT_OK) goto failed;
    r->width = im->width; r->height = im->height; r->depth = d->image->depth;
    r->channels = im->alphaPlane ? 4 : 3;
    r->pixels = (const uint16_t *)r->rgb.pixels;
    r->icc = d->image->icc.data; r->icc_size = d->image->icc.size;
    r->primaries = d->image->colorPrimaries; r->transfer = d->image->transferCharacteristics;
    r->alpha_premultiplied = r->rgb.alphaPremultiplied ? 1 : 0;
    const avifImage *src = d->image;
    unsigned rotation = (src->transformFlags & AVIF_TRANSFORM_IROT) ? src->irot.angle : 0;
    // HEIF applies anticlockwise rotation before mirroring about a display axis.
    const uint32_t rotations[4] = {1,8,3,6};
    const uint32_t flip_vertical[4] = {4,5,2,7};
    const uint32_t flip_horizontal[4] = {2,7,4,5};
    r->orientation = (src->transformFlags & AVIF_TRANSFORM_IMIR)
        ? (src->imir.axis ? flip_horizontal : flip_vertical)[rotation & 3] : rotations[rotation & 3];
    *out = r;
    return NULL;
failed:
    grainy_avif_close(r);
    return avifResultToString(status);
}

int grainy_avif_copy(const GrainyAvif *r, float *dst, size_t count) {
    const size_t n = (size_t)r->width * r->height;
    if (count != n * r->channels) return 0;
    double matrix[3][3];
    avifTransferFunction to_linear = NULL;
    if (!r->icc_size) {
        const avifColorPrimaries primaries = r->primaries == AVIF_COLOR_PRIMARIES_UNSPECIFIED ? AVIF_COLOR_PRIMARIES_BT709 : (avifColorPrimaries)r->primaries;
        const avifTransferCharacteristics transfer = r->transfer == AVIF_TRANSFER_CHARACTERISTICS_UNSPECIFIED ? AVIF_TRANSFER_CHARACTERISTICS_SRGB : (avifTransferCharacteristics)r->transfer;
        if (!avifColorPrimariesComputeRGBToRGBMatrix(primaries, AVIF_COLOR_PRIMARIES_BT709, matrix)) return 0;
        to_linear = avifTransferCharacteristicsGetGammaToLinearFunction(transfer);
    }
    for (uint32_t y = 0; y < r->height; y++) {
        const uint16_t *row = (const uint16_t *)(r->rgb.pixels + (size_t)y * r->rgb.rowBytes);
        for (uint32_t x = 0; x < r->width; x++, row += r->channels, dst += r->channels) {
            for (uint32_t c = 0; c < r->channels; c++) dst[c] = row[c] / 65535.0f;
            if (to_linear) {
                for (size_t c = 0; c < 3; c++) dst[c] = to_linear(dst[c]);
                avifLinearRGBConvertColorSpace(dst, matrix);
            }
        }
    }
    return 1;
}
