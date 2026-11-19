// Port of native_gpu.develop_resident's grain addition and final clip.
@compute @workgroup_size(16,16,1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w=u32(p[0]);let h=u32(p[1]);if id.x>=w || id.y>=h {return;}
    let pixel=id.y*w+id.x;let index=pixel*3u;var a=load_rgb(index);
    if p[2]!=0.0 {a+=aux[pixel];}
    if p[3]==0.0 {a=clip(a);}
    store_rgb(index,a);
}
