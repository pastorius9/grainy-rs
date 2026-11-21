// The only ABI surface: no LibRaw structs or C++ exceptions cross into Rust.
// Parameter assignments port rawpy 0.27.1 Params and apply_params (MIT).
#include "libraw/libraw.h"
#include "lcms2.h"
#include <algorithm>
#include <cstring>
#include <new>

struct GrainyMeta {
    unsigned sizes[8]; // raw_h, raw_w, h, w, top, left, ih, iw
    double pixel_aspect;
    int flip, colors;
    unsigned filters, black[4], white, linear_max[4];
    float camera_wb[4], daylight_wb[4], rgb_xyz[12], color_matrix[12];
    float iso, shutter, aperture, focal;
    long long timestamp;
    char color_desc[5], make[64], model[64], serial[64], lens[128];
};

class GrainyRaw : public LibRaw {
public:
    void metadata(GrainyMeta* m) {
        std::memset(m, 0, sizeof(*m));
        const auto& s = imgdata.sizes; const auto& c = imgdata.rawdata.color;
        unsigned sz[] = {s.raw_height,s.raw_width,s.height,s.width,s.top_margin,s.left_margin,s.iheight,s.iwidth};
        std::copy(sz,sz+8,m->sizes); m->pixel_aspect=s.pixel_aspect; m->flip=s.flip;
        m->colors=imgdata.idata.colors; m->filters=imgdata.idata.filters;
        std::memcpy(m->camera_wb,c.cam_mul,sizeof(m->camera_wb));
        std::memcpy(m->daylight_wb,c.pre_mul,sizeof(m->daylight_wb));
        std::memcpy(m->rgb_xyz,c.cam_xyz,sizeof(m->rgb_xyz));
        std::memcpy(m->color_matrix,c.cmatrix,sizeof(m->color_matrix));
        m->white=c.maximum; std::memcpy(m->linear_max,c.linear_max,sizeof(m->linear_max));
        // rawpy adjust_bl_: adjust a scratch copy, restoring the processing state.
        auto saved=imgdata.color; imgdata.color=c; adjust_bl();
        for(int i=0;i<4;i++)m->black[i]=imgdata.color.cblack[i];
        imgdata.color=saved;
        std::memcpy(m->color_desc,imgdata.idata.cdesc,4);
        std::strncpy(m->make,imgdata.idata.make,63); std::strncpy(m->model,imgdata.idata.model,63);
        std::strncpy(m->serial,imgdata.shootinginfo.BodySerial,63);
        std::strncpy(m->lens,imgdata.lens.Lens,127);
        m->iso=imgdata.other.iso_speed; m->shutter=imgdata.other.shutter;
        m->aperture=imgdata.other.aperture; m->focal=imgdata.other.focal_len;
        m->timestamp=static_cast<long long>(imgdata.other.timestamp);
    }
};
extern "C" {
const char* gr_version() { return LibRaw::version(); }
const char* gr_error(int code) { return libraw_strerror(code); }
void* gr_open(const void* path, GrainyMeta* meta, int* error) {
    GrainyRaw* raw=nullptr;
    try {
        raw=new GrainyRaw;
#ifdef _WIN32
        *error=raw->open_file(static_cast<const wchar_t*>(path));
#else
        *error=raw->open_file(static_cast<const char*>(path));
#endif
        if(!*error)*error=raw->unpack();
        if(*error){delete raw;return nullptr;}
        raw->metadata(meta);return raw;
    } catch (const std::bad_alloc&) {delete raw;*error=LIBRAW_UNSUFFICIENT_MEMORY;return nullptr;}
      catch (...) {delete raw;*error=LIBRAW_UNSPECIFIED_ERROR;return nullptr;}
}
void gr_close(void* handle) { delete static_cast<GrainyRaw*>(handle); }
void gr_free_image(void* image) { LibRaw::dcraw_clear_mem(static_cast<libraw_processed_image_t*>(image)); }
void* gr_process(void* handle,int mode,int half,const unsigned short** pixels,unsigned* h,unsigned* w,unsigned* channels,int* error) {
    try {
        auto raw=static_cast<GrainyRaw*>(handle); auto& p=raw->imgdata.params;
        p.user_qual=-1;p.half_size=half;p.four_color_rgb=0;p.dcb_iterations=0;p.dcb_enhance_fl=0;
        p.fbdd_noiserd=0;p.threshold=0;p.med_passes=0;p.use_camera_wb=(mode!=0);p.use_auto_wb=0;
        for(int i=0;i<4;i++)p.user_mul[i]=mode==0?1.f:0.f;
        p.output_color=mode;p.output_bps=16;p.user_flip=-1;p.user_black=-1;p.user_sat=-1;
        p.no_auto_bright=1;p.no_auto_scale=0;p.auto_bright_thr=LIBRAW_DEFAULT_AUTO_BRIGHTNESS_THRESHOLD;
        p.adjust_maximum_thr=mode==0?0.f:.75f;p.bright=1;p.highlight=0;
        p.exp_correc=-1;p.exp_shift=1;p.exp_preser=0;p.gamm[0]=1;p.gamm[1]=1;p.aber[0]=1;p.aber[2]=1;
        *error=raw->dcraw_process();if(*error)return nullptr;
        auto image=raw->dcraw_make_mem_image(error);if(!image)return nullptr;
        if(image->type!=LIBRAW_IMAGE_BITMAP||image->bits!=16){LibRaw::dcraw_clear_mem(image);*error=LIBRAW_UNSPECIFIED_ERROR;return nullptr;}
        *h=image->height;*w=image->width;*channels=image->colors;
        *pixels=reinterpret_cast<const unsigned short*>(image->data);return image;
    } catch(const std::bad_alloc&){*error=LIBRAW_UNSUFFICIENT_MEMORY;return nullptr;}
      catch(...){*error=LIBRAW_UNSPECIFIED_ERROR;return nullptr;}
}
} // extern "C"

// Profiles are serialised once and reopened per thread with an isolated cmsContext,
// matching imagecodecs.cms_profile's ICC fixed-point quantisation before native_color creates its float transform.
namespace {
struct ProPhotoSrgbProfiles {
    unsigned char ab[4096], bb[4096];
    cmsUInt32Number al = 0, bl = 0;
    bool ok = false;
    ProPhotoSrgbProfiles() {
        cmsContext ctx = cmsCreateContext(nullptr, nullptr);
        if (!ctx) return;
        cmsCIExyY d50 = {.3457, .3585, 1.};
        cmsCIExyYTRIPLE pro = {{.7347, .2653, 1.}, {.1596, .8404, 1.}, {.0366, .0001, 1.}};
        cmsCIExyY d65 = {.3127, .3290, 1.};
        cmsCIExyYTRIPLE srgb = {{.64, .33, 1.}, {.30, .60, 1.}, {.15, .06, 1.}};
        cmsToneCurve* curve = cmsBuildGamma(ctx, 1.);
        if (!curve) {
            cmsDeleteContext(ctx);
            return;
        }
        cmsToneCurve* curves[] = {curve, curve, curve};
        auto a = cmsCreateRGBProfileTHR(ctx, &d50, &pro, curves);
        auto b = cmsCreateRGBProfileTHR(ctx, &d65, &srgb, curves);
        cmsFreeToneCurve(curve);
        if (a && b) {
            al = sizeof(ab);
            bl = sizeof(bb);
            ok = cmsSaveProfileToMem(a, ab, &al) && cmsSaveProfileToMem(b, bb, &bl);
        }
        if (a) cmsCloseProfile(a);
        if (b) cmsCloseProfile(b);
        cmsDeleteContext(ctx);
    }
};

struct ProPhotoSrgbTransform {
    cmsContext ctx = nullptr;
    cmsHTRANSFORM transform = nullptr;
    ProPhotoSrgbTransform() {
        static const ProPhotoSrgbProfiles profiles;
        if (!profiles.ok) return;
        ctx = cmsCreateContext(nullptr, nullptr);
        if (!ctx) return;
        auto a = cmsOpenProfileFromMemTHR(ctx, profiles.ab, profiles.al);
        auto b = cmsOpenProfileFromMemTHR(ctx, profiles.bb, profiles.bl);
        if (a && b) {
            transform = cmsCreateTransformTHR(ctx, a, TYPE_RGB_FLT, b, TYPE_RGB_FLT, 1, cmsFLAGS_NOCACHE);
        }
        if (a) cmsCloseProfile(a);
        if (b) cmsCloseProfile(b);
    }
    ~ProPhotoSrgbTransform() {
        if (transform) cmsDeleteTransform(transform);
        if (ctx) cmsDeleteContext(ctx);
    }
};
} // namespace

extern "C" int gr_prophoto_srgb(const float* input, float* output, unsigned count) {
    thread_local ProPhotoSrgbTransform cached;
    if (!cached.transform) return 1;
    cmsDoTransform(cached.transform, input, output, count);
    return 0;
}
