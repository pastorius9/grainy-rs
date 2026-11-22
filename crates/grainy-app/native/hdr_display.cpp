// Adapted from Grainy (MIT), Copyright (c) 2026 pastorius9.
// Grainy HDR viewport. The OS performs the scRGB-to-monitor conversion.
// No display configuration or calibration is ever changed here.
#define NOMINMAX
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shlobj.h>
#include <d3d11.h>
#include <dxgi1_6.h>
#include <d3dcompiler.h>
#include <wrl/client.h>
#include <vector>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <stdexcept>
#include <memory>
using Microsoft::WRL::ComPtr;
#define API extern "C"
struct Info { unsigned size, supported, active, bits; float white, peak, full, minimum; wchar_t name[32]; };
static void check(HRESULT h) { if(FAILED(h)) throw h; }
static ComPtr<IDXGIAdapter1> output(HMONITOR monitor, Info& info) {
    info={sizeof(Info),0,0,8,80,80,80,0,{0}};
    ComPtr<IDXGIFactory1> factory;check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)));

    for(UINT a=0;;++a) {
        ComPtr<IDXGIAdapter1> adapter;auto adapterResult=factory->EnumAdapters1(a,&adapter);if(adapterResult==DXGI_ERROR_NOT_FOUND) break;check(adapterResult);
        for(UINT o=0;;++o) {
            ComPtr<IDXGIOutput> out;auto outputResult=adapter->EnumOutputs(o,&out);if(outputResult==DXGI_ERROR_NOT_FOUND) break;check(outputResult);
            DXGI_OUTPUT_DESC desc{};check(out->GetDesc(&desc));if(desc.Monitor!=monitor) continue;
            wcsncpy_s(info.name,desc.DeviceName,_TRUNCATE);
            ComPtr<IDXGIOutput6> six;
            if(SUCCEEDED(out.As(&six))) {
                DXGI_OUTPUT_DESC1 d{};check(six->GetDesc1(&d));info.bits=d.BitsPerColor;
                info.active=d.ColorSpace==DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
                info.supported=info.active;info.peak=d.MaxLuminance;info.full=d.MaxFullFrameLuminance;info.minimum=d.MinLuminance;
            }
            for(int retry=0;retry<3;++retry) {
                UINT pc=0,mc=0;if(GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS,&pc,&mc)!=ERROR_SUCCESS) break;
                std::vector<DISPLAYCONFIG_PATH_INFO> paths(pc);std::vector<DISPLAYCONFIG_MODE_INFO> modes(mc);
                auto result=QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS,&pc,paths.data(),&mc,modes.data(),nullptr);
                if(result==ERROR_INSUFFICIENT_BUFFER) continue;if(result!=ERROR_SUCCESS) break;
                for(UINT i=0;i<pc;++i) {
                    const auto& p=paths[i];DISPLAYCONFIG_SOURCE_DEVICE_NAME source{};
                    source.header={DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,sizeof(source),p.sourceInfo.adapterId,p.sourceInfo.id};
                    if(DisplayConfigGetDeviceInfo(&source.header)!=ERROR_SUCCESS || wcscmp(source.viewGdiDeviceName,info.name)) continue;
#if defined(NTDDI_WIN11_GA) && NTDDI_VERSION >= NTDDI_WIN11_GA
                    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO_2 advanced{};
                    advanced.header={DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO_2,sizeof(advanced),p.targetInfo.adapterId,p.targetInfo.id};
                    if(DisplayConfigGetDeviceInfo(&advanced.header)==ERROR_SUCCESS) {
                        info.supported=advanced.highDynamicRangeSupported;
                        info.active=advanced.activeColorMode==DISPLAYCONFIG_ADVANCED_COLOR_MODE_HDR;
                    }
#endif
                    DISPLAYCONFIG_SDR_WHITE_LEVEL white{};
                    white.header={DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,sizeof(white),p.targetInfo.adapterId,p.targetInfo.id};
                    if(DisplayConfigGetDeviceInfo(&white.header)==ERROR_SUCCESS && white.SDRWhiteLevel>0)
                        info.white=80.f*white.SDRWhiteLevel/1000.f;
                    break;
                }
                break;
            }
            if(!info.active) info.white=80.f;
            // Unknown peak must not be advertised as measured HDR headroom.
            if(!std::isfinite(info.peak)||info.peak<info.white)info.peak=info.white;
            return adapter;
        }
    }
    throw HRESULT(DXGI_ERROR_NOT_FOUND);
}
static const char* shader=R"(
Texture2D photo:register(t0); SamplerState samp:register(s0);
cbuffer Params:register(b0) { float4 bounds; float2 viewport; float whiteScale; float peak; };
struct Vertex { float4 position:SV_POSITION; float2 uv:TEXCOORD0; };
Vertex vs(uint id:SV_VertexID) {
    Vertex v;v.uv=float2((id<<1)&2,id&2);v.position=float4(v.uv*float2(2,-2)+float2(-1,1),0,1);return v;
}
float3 decodeSRGB(float3 c) { return lerp(c/12.92,pow(max((c+.055)/1.055,0),2.4),step(.04045,c)); }
float4 ps(Vertex v):SV_TARGET {
    float2 p=v.uv*viewport;float2 uv=(p-bounds.xy)/bounds.zw;
    float3 rgb=decodeSRGB(float3(16,18,21)/255.0);
    if(all(uv>=0)&&all(uv<=1))rgb=photo.Sample(samp,uv).rgb;
    // Clip only for this display. Extended source and export data stay intact.
    rgb=min(rgb,peak)*whiteScale;

    return float4(rgb,1);
})";
struct Renderer {
    HWND hwnd{};UINT width=0,height=0,iw=0,ih=0;DWORD owner=GetCurrentThreadId();
    ComPtr<ID3D11Device> device;ComPtr<ID3D11DeviceContext> context;
    ComPtr<IDXGISwapChain3> swap;ComPtr<ID3D11RenderTargetView> target;
    ComPtr<ID3D11VertexShader> vs;ComPtr<ID3D11PixelShader> ps;
    ComPtr<ID3D11SamplerState> sampler;ComPtr<ID3D11Buffer> params;
    ComPtr<ID3D11Texture2D> photo;ComPtr<ID3D11ShaderResourceView> photoView;
    ComPtr<ID3D11Texture2D> readback;
    Renderer(HWND h,bool warp):hwnd(h) {
        Info info{};auto adapter=warp ? ComPtr<IDXGIAdapter1>{} : output(MonitorFromWindow(h,MONITOR_DEFAULTTONEAREST),info);D3D_FEATURE_LEVEL level{};
        check(D3D11CreateDevice(warp?nullptr:adapter.Get(),warp?D3D_DRIVER_TYPE_WARP:D3D_DRIVER_TYPE_UNKNOWN,
            nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,nullptr,0,D3D11_SDK_VERSION,&device,&level,&context));
        ComPtr<IDXGIDevice> dxgi;check(device.As(&dxgi));ComPtr<IDXGIAdapter> actual;check(dxgi->GetAdapter(&actual));
        ComPtr<IDXGIFactory2> factory;check(actual->GetParent(IID_PPV_ARGS(&factory)));
        DXGI_SWAP_CHAIN_DESC1 desc{};desc.Width=desc.Height=1;desc.Format=DXGI_FORMAT_R16G16B16A16_FLOAT;
        desc.SampleDesc.Count=1;desc.BufferUsage=DXGI_USAGE_RENDER_TARGET_OUTPUT;desc.BufferCount=2;
        desc.SwapEffect=DXGI_SWAP_EFFECT_FLIP_DISCARD;desc.Scaling=DXGI_SCALING_STRETCH;desc.AlphaMode=DXGI_ALPHA_MODE_IGNORE;
        ComPtr<IDXGISwapChain1> first;check(factory->CreateSwapChainForHwnd(device.Get(),h,&desc,nullptr,nullptr,&first));
        check(first.As(&swap));check(factory->MakeWindowAssociation(h,DXGI_MWA_NO_ALT_ENTER));
        UINT support=0;check(swap->CheckColorSpaceSupport(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709,&support));
        if(!(support&DXGI_SWAP_CHAIN_COLOR_SPACE_SUPPORT_FLAG_PRESENT))throw HRESULT(DXGI_ERROR_UNSUPPORTED);
        check(swap->SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709));
        ComPtr<ID3DBlob> code,error;
        check(D3DCompile(shader,strlen(shader),nullptr,nullptr,nullptr,"vs","vs_4_0",D3DCOMPILE_OPTIMIZATION_LEVEL3,0,&code,&error));
        check(device->CreateVertexShader(code->GetBufferPointer(),code->GetBufferSize(),nullptr,&vs));code.Reset();error.Reset();
        check(D3DCompile(shader,strlen(shader),nullptr,nullptr,nullptr,"ps","ps_4_0",D3DCOMPILE_OPTIMIZATION_LEVEL3,0,&code,&error));
        check(device->CreatePixelShader(code->GetBufferPointer(),code->GetBufferSize(),nullptr,&ps));
        D3D11_SAMPLER_DESC sd{};sd.Filter=D3D11_FILTER_MIN_MAG_MIP_LINEAR;sd.AddressU=sd.AddressV=sd.AddressW=D3D11_TEXTURE_ADDRESS_CLAMP;sd.MaxLOD=D3D11_FLOAT32_MAX;
        check(device->CreateSamplerState(&sd,&sampler));
        D3D11_BUFFER_DESC bd{};bd.ByteWidth=32;bd.Usage=D3D11_USAGE_DEFAULT;bd.BindFlags=D3D11_BIND_CONSTANT_BUFFER;
        check(device->CreateBuffer(&bd,nullptr,&params));
    }
    ~Renderer(){if(context){context->ClearState();context->Flush();}}
    void thread(){if(GetCurrentThreadId()!=owner)throw HRESULT(RPC_E_WRONG_THREAD);}
    void texture(UINT w,UINT h,DXGI_FORMAT format,ComPtr<ID3D11Texture2D>& tex,ComPtr<ID3D11ShaderResourceView>& view) {
        view.Reset();tex.Reset();D3D11_TEXTURE2D_DESC d{};d.Width=w;d.Height=h;d.MipLevels=d.ArraySize=1;
        d.Format=format;d.SampleDesc.Count=1;d.Usage=D3D11_USAGE_DEFAULT;d.BindFlags=D3D11_BIND_SHADER_RESOURCE;
        check(device->CreateTexture2D(&d,nullptr,&tex));check(device->CreateShaderResourceView(tex.Get(),nullptr,&view));
    }
    void upload(const float* pixels,UINT w,UINT h) {
        thread();if(!pixels||!w||!h||w>16384||h>16384)throw HRESULT(E_INVALIDARG);
        if(w!=iw||h!=ih){texture(w,h,DXGI_FORMAT_R32G32B32A32_FLOAT,photo,photoView);iw=w;ih=h;}
        context->UpdateSubresource(photo.Get(),0,nullptr,pixels,w*16,0);
    }
    void draw(UINT w,UINT h,const float* rectangle,float white,float maxNits,bool present) {
        thread();if(!photo||!w||!h||w>16384||h>16384||!rectangle||white<=0||maxNits<white)throw HRESULT(E_INVALIDARG);
        if(w!=width||h!=height) {
            context->OMSetRenderTargets(0,nullptr,nullptr);target.Reset();readback.Reset();
            check(swap->ResizeBuffers(0,w,h,DXGI_FORMAT_UNKNOWN,0));
            ComPtr<ID3D11Texture2D> back;check(swap->GetBuffer(0,IID_PPV_ARGS(&back)));
            check(device->CreateRenderTargetView(back.Get(),nullptr,&target));
            width=w;height=h;
        }

        float constants[8]={rectangle[0],rectangle[1],rectangle[2],rectangle[3],float(w),float(h),white/80.f,maxNits/white};
        context->UpdateSubresource(params.Get(),0,nullptr,constants,0,0);
        auto rt=target.Get();context->OMSetRenderTargets(1,&rt,nullptr);
        D3D11_VIEWPORT vp{0,0,float(w),float(h),0,1};context->RSSetViewports(1,&vp);
        context->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context->VSSetShader(vs.Get(),nullptr,0);context->PSSetShader(ps.Get(),nullptr,0);
        auto cb=params.Get();context->PSSetConstantBuffers(0,1,&cb);
        auto sm=sampler.Get();context->PSSetSamplers(0,1,&sm);
        auto view=photoView.Get();context->PSSetShaderResources(0,1,&view);
        context->Draw(3,0);
        ID3D11ShaderResourceView* none=nullptr;context->PSSetShaderResources(0,1,&none);
        if(present)check(swap->Present(0,0));
    }
};
// Test diagnostic reads our own back buffer only, never another app or desktop.
static int readback(void* renderer,unsigned short* rgba,unsigned count) {
    if(!renderer||!rgba)return E_INVALIDARG;
    try {
        auto r=static_cast<Renderer*>(renderer);r->thread();if(count<r->width*r->height*4)return E_INVALIDARG;
        ComPtr<ID3D11Texture2D> back;check(r->swap->GetBuffer(0,IID_PPV_ARGS(&back)));
        if(!r->readback){D3D11_TEXTURE2D_DESC d{};back->GetDesc(&d);d.Usage=D3D11_USAGE_STAGING;d.BindFlags=0;d.CPUAccessFlags=D3D11_CPU_ACCESS_READ;d.MiscFlags=0;check(r->device->CreateTexture2D(&d,nullptr,&r->readback));}
        r->context->CopyResource(r->readback.Get(),back.Get());D3D11_MAPPED_SUBRESOURCE data{};
        check(r->context->Map(r->readback.Get(),0,D3D11_MAP_READ,0,&data));
        for(UINT y=0;y<r->height;++y)memcpy(rgba+y*r->width*4,static_cast<char*>(data.pData)+y*data.RowPitch,r->width*8);
        r->context->Unmap(r->readback.Get(),0);return S_OK;
    }catch(HRESULT e){return e;}catch(...){return E_FAIL;}
}

namespace {
struct MonitorChoice { HMONITOR found{}; };
BOOL CALLBACK find_hdr(HMONITOR monitor, HDC, LPRECT, LPARAM arg) {
    try {
        Info info{};
        output(monitor, info);
        if (info.active && info.peak > info.white) {
            reinterpret_cast<MonitorChoice*>(arg)->found = monitor;
            return FALSE;
        }
    } catch (...) {}
    return TRUE;
}

struct Viewer {
    HWND window{};
    std::unique_ptr<Renderer> renderer;
    Info info{};
    HRESULT error{S_OK};
    bool closed{false};
    bool monitor_dirty{true};

    ~Viewer() {
        renderer.reset();
        if (window) DestroyWindow(window);
    }
    void monitor() {
        output(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), info);
        if (!info.active || info.peak <= info.white) throw HRESULT(DXGI_ERROR_UNSUPPORTED);
        monitor_dirty = false;
    }
    void paint() {
        if (!renderer || !renderer->photo || !IsWindowVisible(window) || IsIconic(window)) return;
        if (monitor_dirty) monitor();
        RECT bounds{};
        if (!GetClientRect(window, &bounds) || !bounds.right || !bounds.bottom) return;
        float width = float(bounds.right), height = float(bounds.bottom);
        float scale = std::min(width / renderer->iw, height / renderer->ih);
        float w = scale * renderer->iw, h = scale * renderer->ih;
        float fitted[] = {(width - w) * .5f, (height - h) * .5f, w, h};
        renderer->draw(UINT(bounds.right), UINT(bounds.bottom), fitted, info.white, info.peak, true);
    }
    static LRESULT CALLBACK procedure(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) noexcept {
        auto self = reinterpret_cast<Viewer*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
        if (msg == WM_NCCREATE) {
            self = static_cast<Viewer*>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams);
            self->window = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
        }
        if (!self) return DefWindowProcW(hwnd, msg, wp, lp);
        switch (msg) {
            case WM_CLOSE:
            case WM_KEYDOWN:
                if (msg == WM_CLOSE || wp == VK_ESCAPE) {
                    self->closed = true;
                    ShowWindow(hwnd, SW_HIDE);
                    return 0;
                }
                break;
            case WM_WINDOWPOSCHANGED:
            case WM_DISPLAYCHANGE:
            case WM_SETTINGCHANGE:
                self->monitor_dirty = true;
                InvalidateRect(hwnd, nullptr, FALSE);
                break;
            case WM_SIZE:
                InvalidateRect(hwnd, nullptr, FALSE);
                return 0;
            case WM_PAINT: {
                PAINTSTRUCT paint{};
                BeginPaint(hwnd, &paint);
                try { self->paint(); }
                catch (HRESULT e) { self->error = e; }
                catch (...) { self->error = E_FAIL; }
                EndPaint(hwnd, &paint);
                if (FAILED(self->error)) ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            case WM_NCDESTROY:
                self->closed = true;
                self->window = nullptr;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                break;
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    void open(HWND owner) {
        if (!IsWindow(owner)) throw HRESULT(E_INVALIDARG);
        MonitorChoice choice{};
        find_hdr(MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST), nullptr, nullptr, reinterpret_cast<LPARAM>(&choice));
        if (!choice.found) EnumDisplayMonitors(nullptr, nullptr, find_hdr, reinterpret_cast<LPARAM>(&choice));
        if (!choice.found) throw HRESULT(DXGI_ERROR_UNSUPPORTED);
        MONITORINFO target{sizeof(target)};
        if (!GetMonitorInfoW(choice.found, &target)) throw HRESULT(E_FAIL);
        auto instance = GetModuleHandleW(nullptr);
        WNDCLASSW cls{};
        cls.lpfnWndProc = procedure;
        cls.hInstance = instance;
        cls.lpszClassName = L"GrainyScRGBPhotoViewer";
        cls.hIcon = LoadIconW(instance, MAKEINTRESOURCEW(1));
        cls.hCursor = LoadCursorW(nullptr, MAKEINTRESOURCEW(32512));
        cls.hbrBackground = static_cast<HBRUSH>(GetStockObject(BLACK_BRUSH));
        if (!RegisterClassW(&cls) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) throw HRESULT(E_FAIL);
        RECT r = target.rcWork;
        int w = std::min(1280L, (r.right - r.left) * 4 / 5);
        int h = std::min(900L, (r.bottom - r.top) * 4 / 5);
        if (!CreateWindowExW(0, cls.lpszClassName, L"Grainy HDR", WS_OVERLAPPEDWINDOW,
                r.left + (r.right-r.left-w)/2, r.top + (r.bottom-r.top-h)/2, w, h,
                owner, nullptr, instance, this)) throw HRESULT(E_FAIL);
        monitor();
        renderer = std::make_unique<Renderer>(window, false);
    }
};
}

API HRESULT grainy_hdr_view_open(HWND owner, Viewer** out) noexcept {
    if (!out) return E_INVALIDARG;
    *out = nullptr;
    try {
        auto viewer = std::make_unique<Viewer>();
        viewer->open(owner);
        *out = viewer.release();
        return S_OK;
    } catch (HRESULT e) { return e; } catch (...) { return E_FAIL; }
}
API void grainy_hdr_view_drop(Viewer* viewer) noexcept { delete viewer; }
API HRESULT grainy_hdr_view_state(Viewer* viewer, UINT* w, UINT* h, float* stops) noexcept {
    if (!viewer || !w || !h || !stops) return E_INVALIDARG;
    if (FAILED(viewer->error)) return viewer->error;
    if (viewer->closed || !viewer->window) return S_FALSE;
    RECT r{};
    if (!GetClientRect(viewer->window, &r)) return E_FAIL;
    *w = UINT(r.right); *h = UINT(r.bottom);
    *stops = std::log2(viewer->info.peak / viewer->info.white);
    return S_OK;
}
API HRESULT grainy_hdr_view_frame(Viewer* viewer, const float* rgba, UINT w, UINT h) noexcept {
    if (!viewer || !viewer->window || viewer->closed) return E_INVALIDARG;
    try {
        if (!rgba) {
            ShowWindow(viewer->window, SW_HIDE);
        } else {
            viewer->renderer->upload(rgba, w, h);
            if (!IsWindowVisible(viewer->window)) ShowWindow(viewer->window, SW_SHOWNOACTIVATE);
            viewer->paint();
        }
        return S_OK;
    } catch (HRESULT e) { return e; } catch (...) { return E_FAIL; }
}

// Exercise the same shaders and FP16 swap chain in a hidden WARP window without changing any display setting.
API HRESULT grainy_hdr_test_readback(unsigned short* samples) noexcept {
    HWND window = CreateWindowExW(0, L"STATIC", L"", WS_POPUP, 0, 0, 4, 1, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
    if (!window || !samples) return E_FAIL;
    HRESULT result = S_OK;
    try {
        Renderer gpu(window, true);
        const float input[] = {0.5f,0,0,1, 1,0,0,1, 2,0,0,1, 4,0,0,1};
        const float rect[] = {0,0,4,1};
        gpu.upload(input, 4, 1);
        gpu.draw(4, 1, rect, 80, 640, false);
        result = readback(&gpu, samples, 16);
    } catch (HRESULT e) { result = e; } catch (...) { result = E_FAIL; }
    DestroyWindow(window);
    return result;
}

// Refresh Explorer's cached icon for a replaced executable after an in-place update.
API void grainy_shell_notify_updated(const wchar_t* path) noexcept {
    if (path && *path) {
        SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW | SHCNF_FLUSHNOWAIT, path, nullptr);
    }
}
