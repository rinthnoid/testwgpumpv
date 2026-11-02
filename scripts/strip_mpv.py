#!/usr/bin/env python3
"""Rewrite mpv's video/out/vo.c for the wgpu-only build.

- removes the `extern const struct vo_driver video_out_X;` declarations
  for every removed legacy VO
- removes their entries from `video_out_drivers[]`, together with the
  surrounding `#if ... #endif` guard when that guard exclusively wraps
  removed entries
- registers `video_out_wgpu` as the first driver (making it the default)

Idempotent: safe to run multiple times. Exit 0 on success.

License: GPL-2.0-or-later. See LICENSE.
"""

import re
import sys

REMOVED = [
    "gpu_next",
    "gpu",
    "vdpau",
    "direct3d",
    "dmabuf_wayland",
    "wlshm",
    "xv",
    "mediacodec_embed",
    "sdl",
    "vaapi",
    "x11",
    "drm",
]

EXTERN_RE = re.compile(r"^extern const struct vo_driver video_out_(\w+);$")
ENTRY_RE = re.compile(r"^(\s*)&video_out_(\w+),\s*$")
ARRAY_START = re.compile(r"static const struct vo_driver \*const video_out_drivers\[\]\s*=")


def drop_externs(lines):
    out = []
    for ln in lines:
        m = EXTERN_RE.match(ln)
        if m and m.group(1) in REMOVED:
            continue
        out.append(ln)
    return out


def matching_endif(lines, start):
    """Index of the #endif matching the #if at `start` (also #ifdef/#ifndef)."""
    depth = 0
    for i in range(start, len(lines)):
        s = lines[i].strip()
        if re.match(r"#\s*if", s):
            depth += 1
        elif re.match(r"#\s*endif", s):
            depth -= 1
            if depth == 0:
                return i
    raise RuntimeError(f"unbalanced #if at line {start + 1}")


def drop_removed_entries(lines):
    """Remove `&video_out_X,` for removed X; collapse guards that end up
    wrapping nothing (or only removed entries). Repeat until stable."""
    changed = True
    while changed:
        changed = False
        # find the array region
        start = None
        for i, ln in enumerate(lines):
            if ARRAY_START.search(ln):
                start = i
                break
        if start is None:
            raise RuntimeError("video_out_drivers[] not found")
        # array end: first line that is exactly '};' after start
        end = None
        for i in range(start, len(lines)):
            if lines[i].strip() == "};":
                end = i
                break
        if end is None:
            raise RuntimeError("video_out_drivers[] not closed")

        body = lines[start + 1 : end]
        # 1. drop removed entry lines
        kept = []
        dropped_any = False
        for ln in body:
            m = ENTRY_RE.match(ln)
            if m and m.group(2) in REMOVED:
                dropped_any = True
                continue
            kept.append(ln)
        if dropped_any:
            lines = lines[: start + 1] + kept + lines[end:]
            changed = True
            continue

        # 2. collapse #if blocks whose entries are all gone
        i = start + 1
        while i < len(lines):
            s = lines[i].strip()
            if re.match(r"#\s*if", s):
                j = matching_endif(lines, i)
                region = lines[i + 1 : j]
                if not any(ENTRY_RE.match(r) for r in region):
                    lines = lines[:i] + lines[j + 1 :]
                    changed = True
                    break
                i = j + 1
            else:
                i += 1
    return lines


def insert_wgpu(lines):
    if any("video_out_wgpu" in ln for ln in lines):
        return lines  # already registered (idempotence)
    # extern declaration after the last existing extern
    last_extern = max(i for i, ln in enumerate(lines) if EXTERN_RE.match(ln))
    lines = (
        lines[: last_extern + 1]
        + ["extern const struct vo_driver video_out_wgpu;"]
        + lines[last_extern + 1 :]
    )
    # first entry in the driver list (after the array's opening brace)
    for i, ln in enumerate(lines):
        if ARRAY_START.search(ln):
            brace = next(j for j in range(i, len(lines)) if lines[j].strip() == "{")
            lines = (
                lines[: brace + 1]
                + ["    &video_out_wgpu, // mpv, rebased on wgpu — the one renderer"]
                + lines[brace + 1 :]
            )
            return lines
    raise RuntimeError("video_out_drivers[] not found")


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    path = sys.argv[1]
    with open(path) as f:
        lines = f.read().split("\n")
    lines = drop_externs(lines)
    lines = drop_removed_entries(lines)
    lines = insert_wgpu(lines)
    with open(path, "w") as f:
        f.write("\n".join(lines))
    print(f"strip_mpv.py: {path} rewritten (wgpu registered, legacy VOs removed)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
