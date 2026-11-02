//! CPU-only WGSL gate: every shader must parse and validate with the same
//! naga front-end that wgpu uses at runtime.
//!
//! This exists because shader compilation otherwise happens only on a
//! device: the 0.1.0/0.1.1 releases shipped a player that panicked at
//! startup because a WGSL parameter was named `filter` — a reserved
//! keyword — and no GPU-less machine ever noticed. Parse + validate here
//! catches reserved words, syntax and type errors on any machine, no
//! adapter required.

const VIDEO_BLIT: &str = include_str!("../shaders/video_blit.wgsl");
const OSD_BLEND: &str = include_str!("../shaders/osd_blend.wgsl");

fn check(name: &str, src: &str) {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("{name}: WGSL parse error: {e:#}"));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{name}: WGSL validation error: {e:#}"));
    let _ = info;
    // The renderer drives two stages; a validated module must expose both
    // entry points, otherwise the shader drifted away from what lib.rs
    // expects.
    assert!(
        module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex),
        "{name}: missing vertex entry point vs_main"
    );
    assert!(
        module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment),
        "{name}: missing fragment entry point fs_main"
    );
}

#[test]
fn video_blit_parses_and_validates() {
    check("video_blit.wgsl", VIDEO_BLIT);
}

#[test]
fn osd_blend_parses_and_validates() {
    check("osd_blend.wgsl", OSD_BLEND);
}
