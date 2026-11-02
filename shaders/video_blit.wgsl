// video_blit.wgsl — the one video path: YUV planes → RGB, scaled, dithered.
//
// Single pass. Luma and chroma planes are uint textures; samples are
// normalized with the configured bit depth (8/10-bit, optional right-shift
// for P010-style storage). Filtering is done in-shader with textureLoad so
// that every bit depth is exact and no CPU-side conversion pass exists.
//
// Uniform layout must match `src/params.rs::Params` (8 x vec4 = 128 bytes);
// drift is caught by tests/uniform_layout.rs.

struct Params {
    mat0: vec4<f32>,  // row 0 of A   (offset 0)
    mat1: vec4<f32>,  // row 1 of A   (offset 16)
    mat2: vec4<f32>,  // row 2 of A   (offset 32)
    bias: vec4<f32>,  // rgb bias     (offset 48)
    src:  vec4<f32>,  // y_w, y_h, c_w, c_h (offset 64)
    misc: vec4<f32>,  // depth_y, depth_c, shift_bits, filter (offset 80)
    mode: vec4<f32>,  // interleaved, dither_lsb, parity, 0 (offset 96)
    rect: vec4<f32>,  // video dst rect, fractions of target (offset 112)
};                    // size 128

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var tex_y: texture_2d<u32>;
@group(0) @binding(2) var tex_u: texture_2d<u32>;
@group(0) @binding(3) var tex_v: texture_2d<u32>;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    // Fullscreen triangle; uv spans [0,1] over the luma plane.
    let xy = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return VSOut(
        vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0),
        uv
    );
}

// Normalize a raw sample: optional right-shift (P010 stores 10-bit values
// in the high bits of a 16-bit word), then divide by the full-scale code.
fn norm(raw: u32, depth: f32, shift: f32) -> f32 {
    let d = u32(depth);
    let s = u32(shift);
    return f32(raw >> s) / f32((1u << d) - 1u);
}

fn clampc(c: i32, size: f32) -> i32 {
    return i32(clamp(f32(c), 0.0, size - 1.0));
}

// Manual bilinear fetch with clamped edges, texel-space.
fn bilinear(tex: texture_2d<u32>, tc: vec2<f32>, size: vec2<f32>, depth: f32, shift: f32) -> f32 {
    let base = tc - vec2<f32>(0.5);
    let f = fract(base);
    let i0 = clampc(i32(floor(base.x)), size.x);
    let j0 = clampc(i32(floor(base.y)), size.y);
    let i1 = clampc(i0 + 1, size.x);
    let j1 = clampc(j0 + 1, size.y);
    let s00 = norm(textureLoad(tex, vec2<i32>(i0, j0), 0).r, depth, shift);
    let s10 = norm(textureLoad(tex, vec2<i32>(i1, j0), 0).r, depth, shift);
    let s01 = norm(textureLoad(tex, vec2<i32>(i0, j1), 0).r, depth, shift);
    let s11 = norm(textureLoad(tex, vec2<i32>(i1, j1), 0).r, depth, shift);
    return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y);
}

// Catmull-Rom (bicubic) 4x4 fetch, texel-space. 16 taps.
fn cr_weights(w: f32) -> vec4<f32> {
    let w2 = w * w;
    let w3 = w2 * w;
    return vec4<f32>(
        -0.5 * w3 + w2 - 0.5 * w,
         1.5 * w3 - 2.5 * w2 + 1.0,
        -1.5 * w3 + 2.0 * w2 + 0.5 * w,
         0.5 * w3 - 0.5 * w2
    );
}

fn catmull_rom(tex: texture_2d<u32>, tc: vec2<f32>, size: vec2<f32>, depth: f32, shift: f32) -> f32 {
    let base = tc - vec2<f32>(0.5);
    let f = fract(base);
    let wx = cr_weights(f.x);
    let wy = cr_weights(f.y);
    let x0 = clampc(i32(floor(base.x)) - 1, size.x);
    let y0 = clampc(i32(floor(base.y)) - 1, size.y);
    var acc = 0.0;
    for (var j = 0; j < 4; j++) {
        let y = clampc(y0 + j, size.y);
        var row = 0.0;
        for (var i = 0; i < 4; i++) {
            let x = clampc(x0 + i, size.x);
            let s = norm(textureLoad(tex, vec2<i32>(x, y), 0).r, depth, shift);
            row = row + wx[i] * s;
        }
        acc = acc + wy[j] * row;
    }
    return acc;
}

fn fetch(tex: texture_2d<u32>, tc: vec2<f32>, size: vec2<f32>, depth: f32, shift: f32, filter: u32) -> f32 {
    if (filter == 0u) {
        let c = vec2<i32>(clampc(i32(floor(tc.x)), size.x), clampc(i32(floor(tc.y)), size.y));
        return norm(textureLoad(tex, c, 0).r, depth, shift);
    } else if (filter == 1u) {
        return bilinear(tex, tc, size, depth, shift);
    } else {
        return catmull_rom(tex, tc, size, depth, shift);
    }
}

// 8x8 ordered Bayer matrix, rows interleaved by frame parity so dithering
// does not alias against interlaced content.
fn bayer(parity: u32) -> array<array<f32, 8>, 8> {
    let even = array<array<f32, 8>, 8>(
        array<f32, 8>( 0.0, 32.0,  8.0, 40.0,  2.0, 34.0, 10.0, 42.0),
        array<f32, 8>(48.0, 16.0, 56.0, 24.0, 50.0, 18.0, 58.0, 26.0),
        array<f32, 8>(12.0, 44.0,  4.0, 36.0, 14.0, 46.0,  6.0, 38.0),
        array<f32, 8>(60.0, 28.0, 52.0, 20.0, 62.0, 30.0, 54.0, 22.0),
        array<f32, 8>( 3.0, 35.0, 11.0, 43.0,  1.0, 33.0,  9.0, 41.0),
        array<f32, 8>(51.0, 19.0, 59.0, 27.0, 49.0, 17.0, 57.0, 25.0),
        array<f32, 8>(15.0, 47.0,  7.0, 39.0, 13.0, 45.0,  5.0, 37.0),
        array<f32, 8>(63.0, 31.0, 55.0, 23.0, 61.0, 29.0, 53.0, 21.0),
    );
    let odd = array<array<f32, 8>, 8>(
        array<f32, 8>(49.0, 17.0, 57.0, 25.0, 51.0, 19.0, 59.0, 27.0),
        array<f32, 8>( 1.0, 33.0,  9.0, 41.0,  3.0, 35.0, 11.0, 43.0),
        array<f32, 8>(61.0, 29.0, 53.0, 21.0, 63.0, 31.0, 55.0, 23.0),
        array<f32, 8>(13.0, 45.0,  5.0, 37.0, 15.0, 47.0,  7.0, 39.0),
        array<f32, 8>(50.0, 18.0, 58.0, 26.0, 48.0, 16.0, 56.0, 24.0),
        array<f32, 8>( 2.0, 34.0, 10.0, 42.0,  0.0, 32.0,  8.0, 40.0),
        array<f32, 8>(62.0, 30.0, 54.0, 22.0, 60.0, 28.0, 52.0, 20.0),
        array<f32, 8>(14.0, 46.0,  6.0, 38.0, 12.0, 44.0,  4.0, 36.0),
    );
    return select(even, odd, parity == 1u);
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    // Destination rect (fractions of the target surface). The fullscreen
    // triangle gives us `in.uv` = position on the target in [0,1]; video
    // lives inside the rect, everything outside is black (letterbox).
    // This is how the host expresses aspect ratio / panscan.
    let rel = in.uv;
    let rxy = p.rect.xy;
    let rwh = p.rect.zw;
    let inside = select(
        vec2<bool>(false, false),
        vec2<bool>(true, true),
        (rel.x >= rxy.x) & (rel.x < rxy.x + rwh.x) & (rel.y >= rxy.y) & (rel.y < rxy.y + rwh.y)
    );
    if (!inside.x || !inside.y || rwh.x <= 0.0 || rwh.y <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let video_uv = (rel - rxy) / rwh;

    let filter = u32(p.misc.w);
    let depth_y = p.misc.x;
    let depth_c = p.misc.y;
    let shift = p.misc.z;

    // Texel-space coordinates. Pixel i center sits at i+0.5 in luma space;
    // chroma is co-sited: j = (i + 0.5) * ratio - 0.5.
    let ytc = video_uv * p.src.xy;
    let ratio = p.src.zw / p.src.xy;
    let ctc = (ytc + vec2<f32>(0.5)) * ratio;

    let y = fetch(tex_y, ytc, p.src.xy, depth_y, shift, filter);

    var u: f32;
    var v: f32;
    if (u32(p.mode.x) == 1u) {
        // Interleaved chroma (NV12/P010): U in .r, V in .g.
        let base = ctc - vec2<f32>(0.5);
        let f = fract(base);
        let i0 = clampc(i32(floor(base.x)), p.src.z);
        let j0 = clampc(i32(floor(base.y)), p.src.w);
        let i1 = clampc(i0 + 1, p.src.z);
        let j1 = clampc(j0 + 1, p.src.w);
        let s00 = textureLoad(tex_u, vec2<i32>(i0, j0), 0);
        let s10 = textureLoad(tex_u, vec2<i32>(i1, j0), 0);
        let s01 = textureLoad(tex_u, vec2<i32>(i0, j1), 0);
        let s11 = textureLoad(tex_u, vec2<i32>(i1, j1), 0);
        let ur = mix(mix(f32(s00.r), f32(s10.r), f.x), mix(f32(s01.r), f32(s11.r), f.x), f.y);
        let vr = mix(mix(f32(s00.g), f32(s10.g), f.x), mix(f32(s01.g), f32(s11.g), f.x), f.y);
        u = norm(u32(ur), depth_c, shift);
        v = norm(u32(vr), depth_c, shift);
    } else {
        u = fetch(tex_u, ctc, p.src.zw, depth_c, shift, filter);
        v = fetch(tex_v, ctc, p.src.zw, depth_c, shift, filter);
    }

    var rgb = vec3<f32>(
        p.mat0.x * y + p.mat0.y * u + p.mat0.z * v + p.bias.x,
        p.mat1.x * y + p.mat1.y * u + p.mat1.z * v + p.bias.y,
        p.mat2.x * y + p.mat2.y * u + p.mat2.z * v + p.bias.z
    );

    // Ordered dithering before quantization to the output bit depth.
    let amp = p.mode.y;
    if (amp > 0.0) {
        let m = bayer(u32(p.mode.z));
        let px = vec2<i32>(in.pos.xy);
        let d = (m[px.y % 8][px.x % 8] / 64.0 - 0.5) * amp;
        rgb = rgb + vec3<f32>(d);
    }

    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
