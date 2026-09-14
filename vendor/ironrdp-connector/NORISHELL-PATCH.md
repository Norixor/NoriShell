# NoriShell patch: surface capability negotiation

Base: the official crates.io `ironrdp-connector` 0.10.0 package used by
`ironrdp` 0.17.0. Upstream revision from `.cargo_vcs_info.json`:
`11a0810cfbbabd8b8023875a05e3041216d4b01b`.
Crates.io archive SHA-256: `d5898b3f1fcaca0f9b923b1463e158aeb64dfdec4ac361b7c086492466ff341c`.
The package version, original manifests, and MIT/Apache-2.0 licenses are retained.

## Problem and change

The original `create_client_confirm_active` advertised fast-path output,
Surface Commands, RemoteFX, and frame acknowledgment unconditionally, even when
Demand Active did not offer the associated transport/surface support. With
xrdp 0.9.24 and fast-path disabled, this selected its RFX capture path while the
client received only a black background and pointer.

Before replacing server capabilities with client capabilities, the shared
activation sequence now computes the supported intersection:

- Fast-path output requires the server General `FASTPATH_OUTPUT_SUPPORTED` flag.
- Surface flags are the server/client intersection, empty without fast-path.
- RemoteFX requires `STREAM_SURFACE_BITS`; other existing surface codecs require
  `SET_SURFACE_BITS`. This client does not advertise bitmap-cache drawing orders.
- Frame acknowledgment requires `FRAME_MARKER` and the server Frame Acknowledge
  capability. The existing 20-frame upstream resize workaround is preserved.
- The ordinary Bitmap capability and existing bitmap decoder remain available.
  Normal servers retain RemoteFX; no global codec disable or wire interception
  is introduced. TLS, certificate approval, authentication, and I/O limits are
  unchanged.

This function is shared by first activation and the factory-created reactivation
sequence. Its two tests drive encoded Demand Active through both entry points
and decode the resulting Confirm Active, including support withdrawal, partial
surface flags, absent capabilities, and restoration of RemoteFX. `[lib] test`
is enabled in the normalized Cargo manifest to execute these regression tests.

## Protocol references

- [MS-RDPBCGR 2.2.7.1.1 General](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/41dc6845-07dc-4af6-bc14-d8281acd4877)
- [MS-RDPBCGR 2.2.7.2.9 Surface Commands](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/aa953018-c0a8-4761-bb12-86586c2cd56a)
- [MS-RDPRFX 1.5 prerequisites](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdprfx/516f1a1c-20ec-4ea1-b92a-7995d8a322cf)
- [MS-RDPRFX 2.2.1.3 Frame Acknowledge](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdprfx/e4d498fd-822b-408d-b8b3-1c216f21265b)

## Validation and retirement

Run `cargo test -p ironrdp-connector --lib` from the application workspace.
Also run the NoriShell RDP client tests and the isolated xrdp fixture with
`use_fastpath=none` and `use_fastpath=both`, retaining the authenticated desktop
frame in each mode and restoring `both` afterward. A transport Ready event or
pointer-only frame does not establish successful desktop rendering.

Remove the workspace patch and this vendor copy once an upstream release has
an equivalent negotiation fix and passes these checks. This patch does not add
GDI drawing-order support or slow-path bulk decompression.
