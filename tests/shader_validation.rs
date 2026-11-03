//! Shader gate — the one that must NEVER silently pass.
//!
//! History: release 0.1.1 shipped `video_blit.wgsl` with `filter` as an
//! identifier, which naga 25 reserves. Every local check that mattered was
//! indirect: `cargo check` does not compile WGSL, and the GPU roundtrip
//! tests silently skip where no adapter exists — so the parse error only
//! surfaced on a user's Windows machine at first paint.
//!
//! This test is the first line of defense and it is *device-free*: it runs
//! everywhere, on every commit, and cannot skip. It does exactly what
//! `wgpu::Device::create_shader_module` does before touching a device:
//!
//!   1. PARSE with naga — the same naga version wgpu bundles (cargo unifies
//!      our dev-dependency with wgpu-core's, both resolve to the version in
//!      Cargo.lock; ci.yml additionally asserts there is exactly one naga in
//!      the tree via `cargo tree -i naga`).
//!   2. VALIDATE the module — types, addressing, entry-point signatures.
//!   3. CHECK that every entry point the pipelines reference
//!      (`vs_main`, `fs_main`) actually exists in each module.
//!
//! Any new WGSL file added to `shaders/` must be registered here AND is
//! automatically exercised end-to-end by the GPU roundtrip test
//! (tests/render_roundtrip.rs) wherever an adapter (e.g. lavapipe) runs.

use naga::front::wgsl;

/// Every shader the renderer embeds — keep in sync with `src/lib.rs`
/// (`ensure_pipes`), which loads these via the same `include_str!` paths.
const SHADERS: &[(&str, &str)] = &[
    (
        "bwgpu-video-blit",
        include_str!("../shaders/video_blit.wgsl"),
    ),
    ("bwgpu-osd", include_str!("../shaders/osd_blend.wgsl")),
];

/// Entry points referenced by the render pipelines in `src/lib.rs`.
const ENTRY_POINTS: &[&str] = &["vs_main", "fs_main"];

/// The naga capabilities wgpu derives for a default device (no extra
/// features) — the strictest setting, and what the player binary uses on
/// a fresh device.
fn validator() -> naga::valid::Validator {
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
}

/// Format a parse error with source-anchored diagnostics (line/column,
/// caret), so the failure message reads like the runtime one.
fn parse_error_to_string(err: &naga::front::wgsl::ParseError, src: &str) -> String {
    match err.emit_to_string(src) {
        s if !s.trim().is_empty() => s,
        _ => err.to_string(),
    }
}

#[test]
fn all_shaders_parse_and_validate() {
    let mut failures: Vec<String> = Vec::new();

    for (label, src) in SHADERS {
        // -- 1. parse ---------------------------------------------------
        let module = match wgsl::parse_str(src) {
            Ok(m) => m,
            Err(err) => {
                failures.push(format!(
                    "Shader '{label}' parsing error:\n{}",
                    parse_error_to_string(&err, src)
                ));
                continue; // validation is pointless if parsing failed
            }
        };

        // -- 2. validate ------------------------------------------------
        if let Err(err) = validator().validate(&module) {
            failures.push(format!(
                "Shader '{label}' validation error:\n{}",
                err.emit_to_string(src)
            ));
            continue;
        }

        // -- 3. entry points the pipelines reference must exist ---------
        let have: Vec<&str> = module
            .entry_points
            .iter()
            .map(|ep| ep.name.as_str())
            .collect();
        for ep in ENTRY_POINTS {
            if !have.contains(ep) {
                failures.push(format!(
                    "Shader '{label}' is missing entry point '{ep}' (have: {have:?})"
                ));
            }
        }

        // Every module is a render pass: both stages must be present.
        for stage in [naga::ShaderStage::Vertex, naga::ShaderStage::Fragment] {
            if !module.entry_points.iter().any(|ep| ep.stage == stage) {
                failures.push(format!("Shader '{label}' has no {stage:?} entry point"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} shader(s) failed:\n\n{}",
        failures.len(),
        SHADERS.len(),
        failures.join("\n\n")
    );
}

/// Reserved words are exactly the class of bug that shipped in 0.1.1: the
/// shader parsed fine on an older naga but the runtime's naga refuses
/// them. Guard the identifier vocabulary of the current shaders against
/// the reserved list so a future edit can't reintroduce one quietly.
#[test]
fn no_shader_identifier_is_a_reserved_word() {
    // The reserved words naga's WGSL front-end rejects (naga 25, mirroring
    // the WGSL spec "reserved words" section). Keep this list in sync when
    // bumping wgpu/naga.
    const RESERVED: &[&str] = &[
        "NULL",
        "Self",
        "abstract",
        "active",
        "alignas",
        "alignof",
        "as",
        "asm",
        "asm_fragment",
        "async",
        "attribute",
        "auto",
        "await",
        "become",
        "binding_array",
        "cast",
        "catch",
        "class",
        "co_await",
        "co_return",
        "co_yield",
        "coherent",
        "column_major",
        "common",
        "compile",
        "compile_fragment",
        "concept",
        "const_cast",
        "consteval",
        "constexpr",
        "constinit",
        "crate",
        "debugger",
        "decltype",
        "delete",
        "demote",
        "demote_to_helper",
        "do",
        "dynamic_cast",
        "enum",
        "explicit",
        "export",
        "extends",
        "extern",
        "external",
        "fallthrough",
        "filter",
        "final",
        "finally",
        "friend",
        "from",
        "fxgroup",
        "get",
        "goto",
        "groupshared",
        "highp",
        "impl",
        "implements",
        "import",
        "inline",
        "instanceof",
        "interface",
        "layout",
        "lowp",
        "macro",
        "macro_rules",
        "match",
        "mediump",
        "meta",
        "mod",
        "module",
        "move",
        "mut",
        "mutable",
        "namespace",
        "new",
        "nil",
        "noexcept",
        "noinline",
        "nointerpolation",
        "non_coherent",
        "noncoherent",
        "noperspective",
        "null",
        "nullptr",
        "of",
        "operator",
        "package",
        "packoffset",
        "partition",
        "pass",
        "pixelfragment",
        "precise",
        "precision",
        "premerge",
        "priv",
        "protected",
        "pub",
        "public",
        "readonly",
        "ref",
        "regardless",
        "register",
        "reinterpret_cast",
        "require",
        "resource",
        "restrict",
        "self",
        "set",
        "shared",
        "sizeof",
        "smooth",
        "snorm",
        "static",
        "static_cast",
        "std",
        "subroutine",
        "super",
        "target",
        "template",
        "this",
        "thread_local",
        "throw",
        "trait",
        "try",
        "type",
        "typedef",
        "typeid",
        "typename",
        "typeof",
        "union",
        "unless",
        "unorm",
        "unsafe",
        "unsized",
        "uses",
        "using",
        "varying",
        "virtual",
        "volatile",
        "wgsl",
        "where",
        "while",
        "writeonly",
        "yield",
    ];

    /// Strip `//` and `/* */` comments so prose can't trip the scanner.
    /// (Line/column numbers below refer to the stripped text only; that is
    /// fine, the parse test above reports precise positions.)
    fn strip_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let bytes = src.as_bytes();
        let (mut i, n) = (0usize, bytes.len());
        while i < n {
            if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
            } else if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
        out
    }

    let mut offenders: Vec<String> = Vec::new();
    for (label, src) in SHADERS {
        let code = strip_comments(src);
        let bytes = code.as_bytes();
        for word in RESERVED {
            // Match `word` only where an identifier could appear: not as a
            // substring of a longer identifier.
            let mut from = 0usize;
            while let Some(pos) = code[from..].find(word) {
                let s = from + pos;
                let e = s + word.len();
                let before_ok =
                    s == 0 || !bytes[s - 1].is_ascii_alphanumeric() && bytes[s - 1] != b'_';
                let after_ok =
                    e == bytes.len() || !bytes[e].is_ascii_alphanumeric() && bytes[e] != b'_';
                if before_ok && after_ok {
                    let line = code[..s].matches('\n').count() + 1;
                    offenders.push(format!("{label}:{line}: reserved word '{word}'"));
                    break; // one hit per word per shader is enough
                }
                from = e;
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "reserved words used as identifiers (naga refuses these at runtime):\n{}",
        offenders.join("\n")
    );
}
