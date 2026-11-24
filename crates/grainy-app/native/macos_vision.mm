// Grainy RS macOS native Apple Vision subject & portrait segmentation (Neural Engine / Metal).
// Uses runtime NSProcessInfo + NSClassFromString checks (avoiding @available's compiler-rt
// ___isPlatformVersionAtLeast symbol under -nodefaultlibs) so binaries keep
// MACOSX_DEPLOYMENT_TARGET=11.0 and link only system frameworks (/System/Library/Frameworks).

#import <CoreGraphics/CoreGraphics.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <Vision/Vision.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <vector>
#include <memory>
#include <type_traits>

#define API extern "C"

namespace {

constexpr size_t kSide = 320;

static CGImageRef create_cgimage_from_rgb320(const uint8_t* rgb) {
    std::vector<uint8_t> rgba(kSide * kSide * 4);
    for (size_t i = 0; i < kSide * kSide; ++i) {
        rgba[i * 4 + 0] = rgb[i * 3 + 0];
        rgba[i * 4 + 1] = rgb[i * 3 + 1];
        rgba[i * 4 + 2] = rgb[i * 3 + 2];
        rgba[i * 4 + 3] = 255;
    }
    CFDataRef data = CFDataCreate(kCFAllocatorDefault, rgba.data(), static_cast<CFIndex>(rgba.size()));
    if (!data) return nullptr;
    CGDataProviderRef provider = CGDataProviderCreateWithCFData(data);
    CFRelease(data);
    if (!provider) return nullptr;
    CGColorSpaceRef cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    CGImageRef image = CGImageCreate(
        kSide,
        kSide,
        8,
        32,
        kSide * 4,
        cs,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
        provider,
        nullptr,
        false,
        kCGRenderingIntentDefault
    );
    CGColorSpaceRelease(cs);
    CGDataProviderRelease(provider);
    return image;
}

static bool extract_pixel_buffer_to_u8_320(CVPixelBufferRef buf, uint8_t* out_matte) {
    if (!buf || !out_matte) return false;
    if (CVPixelBufferLockBaseAddress(buf, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        return false;
    }
    struct Unlock {
        CVPixelBufferRef buffer;
        ~Unlock() { CVPixelBufferUnlockBaseAddress(buffer, kCVPixelBufferLock_ReadOnly); }
    } unlock{buf};
    const size_t w = CVPixelBufferGetWidth(buf);
    const size_t h = CVPixelBufferGetHeight(buf);
    const size_t bpr = CVPixelBufferGetBytesPerRow(buf);
    const OSType fmt = CVPixelBufferGetPixelFormatType(buf);
    const uint8_t* base = static_cast<const uint8_t*>(CVPixelBufferGetBaseAddress(buf));
    bool ok = false;
    if (base && w > 0 && h > 0) {
        auto sample_norm = [&](size_t x, size_t y) -> float {
            if (fmt == kCVPixelFormatType_OneComponent32Float) {
                const auto* row = reinterpret_cast<const float*>(base + y * bpr);
                float v = row[x];
                return std::isfinite(v) ? std::clamp(v, 0.0f, 1.0f) : 0.0f;
            }
            if (fmt == kCVPixelFormatType_OneComponent8) {
                return static_cast<float>(base[y * bpr + x]) / 255.0f;
            }
            return 0.0f;
        };
        if (fmt == kCVPixelFormatType_OneComponent32Float || fmt == kCVPixelFormatType_OneComponent8) {
            float lo = 1.0f;
            float hi = 0.0f;
            std::vector<float> grid(kSide * kSide);
            for (size_t y = 0; y < kSide; ++y) {
                float fy = std::clamp((static_cast<float>(y) + 0.5f) * static_cast<float>(h) / static_cast<float>(kSide) - 0.5f,
                                      0.0f, static_cast<float>(h - 1));
                size_t y0 = static_cast<size_t>(std::floor(fy));
                size_t y1 = std::min(y0 + 1, h - 1);
                float dy = fy - static_cast<float>(y0);
                for (size_t x = 0; x < kSide; ++x) {
                    float fx = std::clamp((static_cast<float>(x) + 0.5f) * static_cast<float>(w) / static_cast<float>(kSide) - 0.5f,
                                          0.0f, static_cast<float>(w - 1));
                    size_t x0 = static_cast<size_t>(std::floor(fx));
                    size_t x1 = std::min(x0 + 1, w - 1);
                    float dx = fx - static_cast<float>(x0);
                    float top = sample_norm(x0, y0) * (1.0f - dx) + sample_norm(x1, y0) * dx;
                    float bot = sample_norm(x0, y1) * (1.0f - dx) + sample_norm(x1, y1) * dx;
                    float v = top * (1.0f - dy) + bot * dy;
                    lo = std::min(lo, v);
                    hi = std::max(hi, v);
                    grid[y * kSide + x] = v;
                }
            }
            if (hi - lo > 0.02f) {
                for (size_t i = 0; i < kSide * kSide; ++i) {
                    out_matte[i] = static_cast<uint8_t>(std::lround(std::clamp(grid[i], 0.0f, 1.0f) * 255.0f));
                }
                ok = true;
            }
        }
    }
    return ok;
}

}  // namespace

// mode = 0: Foreground subject/object (macOS 14+ VNGenerateForegroundInstanceMaskRequest)
// mode = 1: Person/portrait prior (macOS 12+ VNGeneratePersonSegmentationRequest)
// Returns 1 if a valid matte was written, 0 if unavailable or no distinct subject found, -1 on error.
static int32_t vision_matte(const uint8_t* rgb320, uint32_t mode, uint8_t* out_matte) {
    if (!rgb320 || !out_matte) return -1;
    @autoreleasepool {
        CGImageRef cgImage = create_cgimage_from_rgb320(rgb320);
        if (!cgImage) return -1;
        std::unique_ptr<std::remove_pointer_t<CGImageRef>, decltype(&CGImageRelease)> imageOwner(cgImage, CGImageRelease);
        int32_t status = 0;
        VNImageRequestHandler* handler = [[VNImageRequestHandler alloc] initWithCGImage:cgImage options:@{}];
        NSError* error = nil;
        NSOperatingSystemVersion ver = [[NSProcessInfo processInfo] operatingSystemVersion];

        if (mode == 0) {
#if defined(__MAC_14_0) && __MAC_OS_X_VERSION_MAX_ALLOWED >= __MAC_14_0
            if (ver.majorVersion >= 14) {
                Class reqClass = NSClassFromString(@"VNGenerateForegroundInstanceMaskRequest");
                if (reqClass != Nil) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
                    VNRequest* req = [[reqClass alloc] init];
                    if (req && [handler performRequests:@[req] error:&error] && req.results.count > 0) {
                        VNInstanceMaskObservation* obs = (VNInstanceMaskObservation*)req.results.firstObject;
                        if (obs && [obs respondsToSelector:@selector(allInstances)] &&
                            [obs respondsToSelector:@selector(generateScaledMaskForImageForInstances:fromRequestHandler:error:)] &&
                            obs.allInstances.count > 0) {
                            CVPixelBufferRef maskBuf =
                                [obs generateScaledMaskForImageForInstances:obs.allInstances
                                                         fromRequestHandler:handler
                                                                      error:&error];
                            if (maskBuf) {
                                std::unique_ptr<std::remove_pointer_t<CVPixelBufferRef>, decltype(&CVPixelBufferRelease)> maskOwner(maskBuf, CVPixelBufferRelease);
                                if (extract_pixel_buffer_to_u8_320(maskBuf, out_matte)) {
                                    status = 1;
                                }
                            }
                        }
                    }
#pragma clang diagnostic pop
                }
            }
#endif
        } else if (mode == 1) {
#if defined(__MAC_12_0) && __MAC_OS_X_VERSION_MAX_ALLOWED >= __MAC_12_0
            if (ver.majorVersion >= 12) {
                Class reqClass = NSClassFromString(@"VNGeneratePersonSegmentationRequest");
                if (reqClass != Nil) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
                    VNGeneratePersonSegmentationRequest* req = [[reqClass alloc] init];
                    if (req) {
                        req.qualityLevel = VNGeneratePersonSegmentationRequestQualityLevelAccurate;
                        req.outputPixelFormat = kCVPixelFormatType_OneComponent8;
                        if ([handler performRequests:@[req] error:&error] && req.results.count > 0) {
                            VNPixelBufferObservation* obs = (VNPixelBufferObservation*)req.results.firstObject;
                            if (obs && obs.pixelBuffer && extract_pixel_buffer_to_u8_320(obs.pixelBuffer, out_matte)) {
                                status = 1;
                            }
                        }
                    }
#pragma clang diagnostic pop
                }
            }
#endif
        }

        return status;
    }
}

API int32_t grainy_macos_vision_matte(const uint8_t* rgb320, uint32_t mode, uint8_t* out_matte) noexcept {
    try {
        @try {
            return vision_matte(rgb320, mode, out_matte);
        } @catch (NSException*) {
            return -1;
        }
    } catch (...) {
        return -1;
    }
}
