// osd_blend.wgsl — OSD/subtitle blending.
//
// OSD bitmaps arrive as straight-alpha RGBA8 (the libass convention), so the
// fragment shader premultiplies and the pipeline blends with
// One / OneMinusSrcAlpha. One quad per bitmap via a per-item uniform block;
// the video underneath stays untouched.

struct Inst {
    dst: vec4<f32>,  // x, y, w, h in target pixels
    uv:  vec4<f32>,  // u0, v0, u1, v1 in texture coords
    tgt: vec4<f32>,  // target width, target height, 0, 0
};                   // size 48

@group(0) @binding(0) var<uniform> inst: Inst;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    // Two triangles covering the unit quad.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];

    // Target pixel rect -> NDC (y-down pixel coords, y-up NDC).
    let px = inst.dst.xy + c * inst.dst.zw;
    let ndc = vec2<f32>(
        px.x / inst.tgt.x * 2.0 - 1.0,
        1.0 - px.y / inst.tgt.y * 2.0
    );
    let uv = mix(inst.uv.xy, inst.uv.zw, c);
    return VSOut(vec4<f32>(ndc, 0.0, 1.0), uv);
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv);
    // Straight alpha -> premultiplied, blended One / OneMinusSrcAlpha.
    return vec4<f32>(c.rgb * c.a, c.a);
}
