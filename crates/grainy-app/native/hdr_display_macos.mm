// Grainy RS macOS native HDR / EDR preview window (Metal RGBA16Float + Extended Linear sRGB).
// Presents extended linear float32 pixels above SDR white on MacBook Pro XDR / Pro Display XDR /
// external HDR monitors without altering any system display configuration or calibration.

#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <memory>

#define API extern "C"

struct GrainyMacHdrViewer;

@interface GrainyHdrWindow : NSWindow
@property(nonatomic, assign) GrainyMacHdrViewer* viewer;
@end

@interface GrainyHdrDelegate : NSObject <NSWindowDelegate>
@property(nonatomic, assign) GrainyMacHdrViewer* viewer;
@end

namespace {

constexpr int32_t kOk = 0;
constexpr int32_t kFalse = 1;
constexpr int32_t kErrUnsupported = static_cast<int32_t>(0x887A0004u);
constexpr int32_t kErrInvalidArg = static_cast<int32_t>(0x80070057u);
constexpr int32_t kErrFail = static_cast<int32_t>(0x80004005u);

struct HdrParams {
    float bounds[4];
    float viewport[2];
    float whiteScale;
    float peak;
};

static NSString* const kMetalShaderSource = @R"(
#include <metal_stdlib>
using namespace metal;

struct Params {
    float4 bounds;
    float2 viewport;
    float whiteScale;
    float peak;
};

struct VertexOut {
    float4 position [[position]];
    float2 uv;
};

vertex VertexOut grainy_hdr_vs(uint vid [[vertex_id]]) {
    VertexOut v;
    v.uv = float2(float((vid << 1u) & 2u), float(vid & 2u));
    v.position = float4(v.uv * float2(2.0f, -2.0f) + float2(-1.0f, 1.0f), 0.0f, 1.0f);
    return v;
}

static inline float3 decode_srgb(float3 c) {
    return select(pow(max((c + 0.055f) / 1.055f, float3(0.0f)), float3(2.4f)), c / 12.92f, c <= 0.04045f);
}

// Software bilinear read on RGBA32Float so all Metal GPUs (Apple Silicon, Intel, AMD, CI
// paravirtualized GPU) work identically without requiring 32-bit float sampler filtering.
static inline float3 sample_bilinear(texture2d<float, access::read> tex, float2 uv) {
    int w = int(tex.get_width());
    int h = int(tex.get_height());
    float fx = clamp(uv.x * float(w) - 0.5f, 0.0f, float(max(w - 1, 0)));
    float fy = clamp(uv.y * float(h) - 0.5f, 0.0f, float(max(h - 1, 0)));
    int x0 = int(floor(fx));
    int y0 = int(floor(fy));
    int x1 = min(x0 + 1, w - 1);
    int y1 = min(y0 + 1, h - 1);
    float tx = fx - float(x0);
    float ty = fy - float(y0);
    float3 c00 = tex.read(uint2(uint(x0), uint(y0))).rgb;
    float3 c10 = tex.read(uint2(uint(x1), uint(y0))).rgb;
    float3 c01 = tex.read(uint2(uint(x0), uint(y1))).rgb;
    float3 c11 = tex.read(uint2(uint(x1), uint(y1))).rgb;
    float3 top = mix(c00, c10, tx);
    float3 bot = mix(c01, c11, tx);
    return mix(top, bot, ty);
}

fragment float4 grainy_hdr_ps(
    VertexOut v [[stage_in]],
    constant Params& p [[buffer(0)]],
    texture2d<float, access::read> photo [[texture(0)]]
) {
    float2 pos = v.uv * p.viewport;
    float2 uv = (pos - p.bounds.xy) / p.bounds.zw;
    float3 rgb = decode_srgb(float3(16.0f, 18.0f, 21.0f) / 255.0f);
    if (uv.x >= 0.0f && uv.x <= 1.0f && uv.y >= 0.0f && uv.y <= 1.0f) {
        rgb = sample_bilinear(photo, uv);
    }
    // Clip only for this display; extended source and export pixels stay intact.
    rgb = min(rgb, float3(p.peak)) * p.whiteScale;
    return float4(rgb, 1.0f);
}
)";

struct MetalRenderer {
    id<MTLDevice> device = nil;
    id<MTLCommandQueue> queue = nil;
    id<MTLRenderPipelineState> pipeline = nil;
    id<MTLTexture> photo = nil;
    uint32_t iw = 0;
    uint32_t ih = 0;

    int32_t init() {
        device = MTLCreateSystemDefaultDevice();
        if (!device) return kErrUnsupported;
        queue = [device newCommandQueue];
        if (!queue) return kErrFail;

        NSError* err = nil;
        id<MTLLibrary> lib = [device newLibraryWithSource:kMetalShaderSource options:nil error:&err];
        if (!lib) return kErrFail;
        id<MTLFunction> vs = [lib newFunctionWithName:@"grainy_hdr_vs"];
        id<MTLFunction> ps = [lib newFunctionWithName:@"grainy_hdr_ps"];
        if (!vs || !ps) return kErrFail;

        MTLRenderPipelineDescriptor* desc = [[MTLRenderPipelineDescriptor alloc] init];
        desc.vertexFunction = vs;
        desc.fragmentFunction = ps;
        desc.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA16Float;
        pipeline = [device newRenderPipelineStateWithDescriptor:desc error:&err];
        if (!pipeline) return kErrFail;
        return kOk;
    }

    // Upload via MTLStorageModeShared buffer -> MTLStorageModePrivate texture so both Apple Silicon
    // and Intel/AMD discrete GPUs work identically without MTLStorageModeManaged.
    int32_t upload(const float* pixels, uint32_t w, uint32_t h) {
        if (!pixels || w == 0 || h == 0 || w > 16384 || h > 16384) return kErrInvalidArg;
        NSUInteger rowBytes = static_cast<NSUInteger>(w) * 16;
        NSUInteger totalBytes = rowBytes * static_cast<NSUInteger>(h);
        if (w != iw || h != ih || !photo) {
            MTLTextureDescriptor* td =
                [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA32Float
                                                                   width:w
                                                                  height:h
                                                               mipmapped:NO];
            td.usage = MTLTextureUsageShaderRead;
            td.storageMode = MTLStorageModePrivate;
            photo = [device newTextureWithDescriptor:td];
            if (!photo) return kErrFail;
            iw = w;
            ih = h;
        }
        // Allocate a transient shared staging buffer per upload so the GPU never reads a buffer
        // being overwritten by a subsequent upload; the command buffer retains it until completion.
        id<MTLBuffer> staging = [device newBufferWithBytes:pixels
                                                    length:totalBytes
                                                   options:MTLResourceStorageModeShared];
        if (!staging) return kErrFail;
        id<MTLCommandBuffer> cmd = [queue commandBuffer];
        if (!cmd) return kErrFail;
        id<MTLBlitCommandEncoder> blit = [cmd blitCommandEncoder];
        [blit copyFromBuffer:staging
                sourceOffset:0
           sourceBytesPerRow:rowBytes
         sourceBytesPerImage:totalBytes
                  sourceSize:MTLSizeMake(w, h, 1)
                   toTexture:photo
            destinationSlice:0
            destinationLevel:0
           destinationOrigin:MTLOriginMake(0, 0, 0)];
        [blit endEncoding];
        [cmd commit];
        return kOk;
    }

    int32_t drawToTexture(
        id<MTLTexture> target,
        uint32_t w,
        uint32_t h,
        const float rect[4],
        float whiteScale,
        float peak,
        id<CAMetalDrawable> presentDrawable,
        id<MTLBuffer> readbackBuf
    ) {
        if (!photo || !target || w == 0 || h == 0 || w > 16384 || h > 16384 || !rect || whiteScale <= 0.0f) {
            return kErrInvalidArg;
        }
        id<MTLCommandBuffer> cmd = [queue commandBuffer];
        if (!cmd) return kErrFail;

        MTLRenderPassDescriptor* pass = [MTLRenderPassDescriptor renderPassDescriptor];
        pass.colorAttachments[0].texture = target;
        pass.colorAttachments[0].loadAction = MTLLoadActionDontCare;
        pass.colorAttachments[0].storeAction = MTLStoreActionStore;

        id<MTLRenderCommandEncoder> enc = [cmd renderCommandEncoderWithDescriptor:pass];
        if (!enc) return kErrFail;
        MTLViewport vp{0.0, 0.0, static_cast<double>(w), static_cast<double>(h), 0.0, 1.0};
        [enc setViewport:vp];
        [enc setRenderPipelineState:pipeline];

        HdrParams params;
        std::memcpy(params.bounds, rect, sizeof(params.bounds));
        params.viewport[0] = static_cast<float>(w);
        params.viewport[1] = static_cast<float>(h);
        params.whiteScale = whiteScale;
        params.peak = peak;
        [enc setFragmentBytes:&params length:sizeof(params) atIndex:0];
        [enc setFragmentTexture:photo atIndex:0];
        [enc drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
        [enc endEncoding];

        if (readbackBuf) {
            id<MTLBlitCommandEncoder> blit = [cmd blitCommandEncoder];
            [blit copyFromTexture:target
                      sourceSlice:0
                      sourceLevel:0
                     sourceOrigin:MTLOriginMake(0, 0, 0)
                       sourceSize:MTLSizeMake(w, h, 1)
                         toBuffer:readbackBuf
                destinationOffset:0
           destinationBytesPerRow:static_cast<NSUInteger>(w) * 8
         destinationBytesPerImage:static_cast<NSUInteger>(w) * static_cast<NSUInteger>(h) * 8];
            [blit endEncoding];
        }
        if (presentDrawable) {
            [cmd presentDrawable:presentDrawable];
        }
        [cmd commit];
        if (readbackBuf) {
            [cmd waitUntilCompleted];
        }
        return kOk;
    }
};

float screen_edr_peak(NSScreen* screen, bool potentialHeadroom = true) {
    if (!screen) return 1.0f;
    float current = static_cast<float>(screen.maximumExtendedDynamicRangeColorComponentValue);
    float potential = current;
    // maximumPotentialExtendedDynamicRangeColorComponentValue was introduced in macOS 12.0;
    // guard with respondsToSelector: so macOS 11 (Big Sur) never raises NSInvalidArgumentException.
    if (potentialHeadroom && [screen respondsToSelector:@selector(maximumPotentialExtendedDynamicRangeColorComponentValue)]) {
        potential = static_cast<float>(screen.maximumPotentialExtendedDynamicRangeColorComponentValue);
    }
    float peak = std::max(current, potential);
    return std::isfinite(peak) && peak > 1.0f ? peak : 1.0f;
}

NSScreen* find_edr_screen(NSWindow* owner) {
    if (owner && owner.screen && screen_edr_peak(owner.screen) > 1.01f) {
        return owner.screen;
    }
    for (NSScreen* screen in [NSScreen screens]) {
        if (screen_edr_peak(screen) > 1.01f) {
            return screen;
        }
    }
    return nil;
}

}  // namespace

struct GrainyMacHdrViewer {
    GrainyHdrWindow* window = nil;
    GrainyHdrDelegate* delegate = nil;
    CAMetalLayer* layer = nil;
    MetalRenderer renderer;
    float peak = 1.0f;
    int32_t error = kOk;
    bool closed = false;

    ~GrainyMacHdrViewer() {
        delegate.viewer = nullptr;
        if (window) {
            window.delegate = nil;
            window.viewer = nil;
            [window orderOut:nil];
            [window close];
            window = nil;
        }
        delegate = nil;
        layer = nil;
    }

    void refreshScreen() {
        NSScreen* screen = window ? window.screen : nil;
        if (!screen) screen = [NSScreen mainScreen];
        peak = screen_edr_peak(screen, false);
        if (layer && window) {
            layer.contentsScale = window.backingScaleFactor > 0.0 ? window.backingScaleFactor : 1.0;
        }
    }

    void paint() {
        if (closed || !window || !layer || !renderer.photo || ![window isVisible] || [window isMiniaturized]) {
            return;
        }
        refreshScreen();
        if (screen_edr_peak(window.screen) <= 1.01f) {
            error = kErrUnsupported;
            [window orderOut:nil];
            return;
        }
        NSView* content = window.contentView;
        if (!content) return;
        NSRect backing = [content convertRectToBacking:content.bounds];
        uint32_t w = static_cast<uint32_t>(std::max(0.0, std::round(backing.size.width)));
        uint32_t h = static_cast<uint32_t>(std::max(0.0, std::round(backing.size.height)));
        if (w == 0 || h == 0) return;

        CGSize desired = CGSizeMake(w, h);
        if (!CGSizeEqualToSize(layer.drawableSize, desired)) {
            layer.drawableSize = desired;
        }
        id<CAMetalDrawable> drawable = [layer nextDrawable];
        if (!drawable) return;

        w = static_cast<uint32_t>(drawable.texture.width);
        h = static_cast<uint32_t>(drawable.texture.height);
        float fw = static_cast<float>(w);
        float fh = static_cast<float>(h);
        float scale = std::min(fw / static_cast<float>(renderer.iw), fh / static_cast<float>(renderer.ih));
        float rw = scale * static_cast<float>(renderer.iw);
        float rh = scale * static_cast<float>(renderer.ih);
        float fitted[4] = {(fw - rw) * 0.5f, (fh - rh) * 0.5f, rw, rh};

        int32_t hr = renderer.drawToTexture(drawable.texture, w, h, fitted, 1.0f, peak, drawable, nil);
        if (hr < 0) {
            error = hr;
            [window orderOut:nil];
        }
    }

    int32_t open(intptr_t ownerViewPtr) {
        if (![NSThread isMainThread]) return kErrFail;
        NSWindow* ownerWindow = nil;
        if (ownerViewPtr != 0) {
            NSView* ownerView = (__bridge NSView*)reinterpret_cast<void*>(ownerViewPtr);
            if (ownerView) ownerWindow = ownerView.window;
        }
        NSScreen* target = find_edr_screen(ownerWindow);
        if (!target) return kErrUnsupported;

        int32_t hr = renderer.init();
        if (hr < 0) return hr;

        NSRect visible = target.visibleFrame;
        CGFloat w = std::min<CGFloat>(1280.0, std::floor(visible.size.width * 0.8));
        CGFloat h = std::min<CGFloat>(900.0, std::floor(visible.size.height * 0.8));
        NSRect frame = NSMakeRect(
            visible.origin.x + std::floor((visible.size.width - w) * 0.5),
            visible.origin.y + std::floor((visible.size.height - h) * 0.5),
            w,
            h
        );
        NSWindowStyleMask style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                  NSWindowStyleMaskResizable | NSWindowStyleMaskMiniaturizable;
        window = [[GrainyHdrWindow alloc] initWithContentRect:frame
                                                    styleMask:style
                                                      backing:NSBackingStoreBuffered
                                                        defer:NO
                                                       screen:target];
        if (!window) return kErrFail;
        window.viewer = this;
        window.title = @"Grainy HDR";
        window.releasedWhenClosed = NO;
        window.backgroundColor = [NSColor colorWithSRGBRed:16.0 / 255.0
                                                     green:18.0 / 255.0
                                                      blue:21.0 / 255.0
                                                     alpha:1.0];

        delegate = [[GrainyHdrDelegate alloc] init];
        delegate.viewer = this;
        window.delegate = delegate;

        layer = [CAMetalLayer layer];
        layer.device = renderer.device;
        layer.pixelFormat = MTLPixelFormatRGBA16Float;
        layer.wantsExtendedDynamicRangeContent = YES;
        layer.framebufferOnly = YES;
        layer.contentsScale = target.backingScaleFactor > 0.0 ? target.backingScaleFactor : 1.0;
        CGColorSpaceRef cs = CGColorSpaceCreateWithName(kCGColorSpaceExtendedLinearSRGB);
        if (cs) {
            layer.colorspace = cs;
            CGColorSpaceRelease(cs);
        }

        NSView* content = window.contentView;
        content.layer = layer;
        content.wantsLayer = YES;
        refreshScreen();
        return kOk;
    }
};

@implementation GrainyHdrWindow
- (void)keyDown:(NSEvent*)event {
    if (event.keyCode == 53 && self.viewer) {  // Escape
        self.viewer->closed = true;
        [self orderOut:nil];
        return;
    }
    [super keyDown:event];
}
@end

@implementation GrainyHdrDelegate
- (BOOL)windowShouldClose:(NSWindow*)sender {
    if (self.viewer) {
        self.viewer->closed = true;
    }
    [sender orderOut:nil];
    return NO;
}
- (void)windowDidResize:(NSNotification*)notification {
    (void)notification;
    if (self.viewer) self.viewer->paint();
}
- (void)windowDidChangeScreen:(NSNotification*)notification {
    (void)notification;
    if (self.viewer) self.viewer->paint();
}
- (void)windowDidChangeBackingProperties:(NSNotification*)notification {
    (void)notification;
    if (self.viewer) self.viewer->paint();
}
@end

API int32_t grainy_hdr_view_open(intptr_t owner, void** out) noexcept {
    if (!out) return kErrInvalidArg;
    *out = nullptr;
    @autoreleasepool {
        @try {
            auto viewer = std::make_unique<GrainyMacHdrViewer>();
            int32_t hr = viewer->open(owner);
            if (hr < 0) return hr;
            *out = viewer.release();
            return kOk;
        } @catch (NSException*) {
            return kErrFail;
        }
    }
}

API void grainy_hdr_view_drop(void* viewer) noexcept {
    @autoreleasepool {
        @try {
            delete static_cast<GrainyMacHdrViewer*>(viewer);
        } @catch (NSException*) {
        }
    }
}

API int32_t grainy_hdr_view_state(void* raw, uint32_t* w, uint32_t* h, float* stops) noexcept {
    if (!raw || !w || !h || !stops) return kErrInvalidArg;
    auto* viewer = static_cast<GrainyMacHdrViewer*>(raw);
    if (viewer->error < 0) return viewer->error;
    if (viewer->closed || !viewer->window) return kFalse;
    @autoreleasepool {
        @try {
            const float previousPeak = viewer->peak;
            viewer->refreshScreen();
            if (viewer->peak != previousPeak) viewer->paint();
            NSView* content = viewer->window.contentView;
            NSRect backing = content ? [content convertRectToBacking:content.bounds] : NSZeroRect;
            *w = static_cast<uint32_t>(std::max(0.0, std::round(backing.size.width)));
            *h = static_cast<uint32_t>(std::max(0.0, std::round(backing.size.height)));
            *stops = std::log2(std::max(1.0f, viewer->peak));
            return kOk;
        } @catch (NSException*) {
            return kErrFail;
        }
    }
}

API int32_t grainy_hdr_view_frame(void* raw, const float* rgba, uint32_t w, uint32_t h) noexcept {
    auto* viewer = static_cast<GrainyMacHdrViewer*>(raw);
    if (!viewer || !viewer->window || viewer->closed) return kErrInvalidArg;
    @autoreleasepool {
        @try {
            if (!rgba) {
                [viewer->window orderOut:nil];
                return kOk;
            }
            int32_t hr = viewer->renderer.upload(rgba, w, h);
            if (hr < 0) return hr;
            if (![viewer->window isVisible]) {
                [viewer->window orderFront:nil];
            }
            viewer->paint();
            return viewer->error < 0 ? viewer->error : kOk;
        } @catch (NSException*) {
            return kErrFail;
        }
    }
}

// Offscreen Metal FP16 readback diagnostic exercising the exact shader without opening a window.
API int32_t grainy_hdr_test_readback(uint16_t* samples) noexcept {
    if (!samples) return kErrInvalidArg;
    @autoreleasepool {
        MetalRenderer gpu;
        int32_t hr = gpu.init();
        if (hr < 0) return hr;
        const float input[16] = {0.5f, 0, 0, 1, 1.0f, 0, 0, 1, 2.0f, 0, 0, 1, 4.0f, 0, 0, 1};
        const float rect[4] = {0.0f, 0.0f, 4.0f, 1.0f};
        hr = gpu.upload(input, 4, 1);
        if (hr < 0) return hr;

        MTLTextureDescriptor* td =
            [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA16Float
                                                               width:4
                                                              height:1
                                                           mipmapped:NO];
        td.usage = MTLTextureUsageRenderTarget;
        td.storageMode = MTLStorageModePrivate;
        id<MTLTexture> target = [gpu.device newTextureWithDescriptor:td];
        id<MTLBuffer> readback = [gpu.device newBufferWithLength:32 options:MTLResourceStorageModeShared];
        if (!target || !readback) return kErrFail;

        hr = gpu.drawToTexture(target, 4, 1, rect, 1.0f, 8.0f, nil, readback);
        if (hr < 0) return hr;
        std::memcpy(samples, readback.contents, 32);
        return kOk;
    }
}
