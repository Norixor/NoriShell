# NoriShell patch: bitmap source scanline stride

Base: official crates.io `ironrdp-session` 0.11.0 used by IronRDP 0.17.0.
Upstream revision: `11a0810cfbbabd8b8023875a05e3041216d4b01b`.
Archive SHA-256: `3f1a5a20c24d8ebf47477f9f513201aeb894e7c7f779ea633ece65e42a9f0652`.
Version, original manifest and MIT/Apache-2.0 license files are retained.

## Defect and change

`fast_path::Processor::process_bitmap_update` decoded a bitmap using its source
`width` and `height`, then passed it directly to image methods that slice rows
using the destination rectangle width. A legal source bitmap wider than the
visible rectangle therefore shifted each row. On xrdp 0.9.24,
slow-path text and window borders visibly tore diagonally. The same incorrect
source-stride assumption was present in both raw and compressed bitmap paths.

The shared Bitmap Update pipeline now normalizes source scanlines before image
application. It respects the declared source width, removes wire byte padding
for raw bitmaps, crops surplus right-hand pixels and bottom-up rows to the
visible top-left rectangle, and validates dimensions and exact byte extent.
Equal source/visible strides borrow the data without a copy. The only extra
allocation when cropping is bounded by the already decoded source data.

This applies equally to planar RDP6, interleaved RLE, and uncompressed bitmap
updates, whether transported through fast-path or slow-path. RemoteFX surface
updates are unchanged. This does not add drawing-order or bulk-decompression
support, alter capability negotiation, or change TLS/authentication.

`[lib] test` is enabled in the normalized Cargo manifest. Tests encode actual
RDP6 planar streams with RLE both enabled and disabled, render cropped rectangles
with distinct per-row pixels, check interleaved RLE and raw 24bpp wire padding, and reject malformed
source extents/lengths before application. Run from the NoriShell workspace:

```
cargo test -p ironrdp-session --lib
cargo test -p norishell-rdp-client --lib --example qa_connect
```

The isolated xrdp fixture must render its authenticated desktop in both
`use_fastpath=none` (bitmap compression enabled) and `use_fastpath=both`, with
retained frame inspection and final restoration to `both`. A nonblack frame
alone does not prove rendering correctness.

Retire this copy and the workspace patch when an upstream release includes the
stride fix and passes these regressions and the real-server check.
