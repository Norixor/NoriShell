<!-- Modified for NoriShell; see the vendor/README.md provenance inventory. -->
# IronRDP RDPSND

RDPSND static channel for audio output implemented as described in [MS-RDPEA].

This crate is part of the [IronRDP] project.

[IronRDP]: https://github.com/Devolutions/IronRDP
[MS-RDPEA]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpea/bea2d5cf-e3b9-4419-92e5-0e074ff9bc5b

## NoriShell local patch (0.9.0)

Source: the crates.io `ironrdp-rdpsnd` 0.9.0 package from Devolutions/IronRDP,
with its original MIT/Apache licenses. Only the client implementation, its unit
tests and the manifest's lib-test switch are changed; the public PDU codecs and
server implementation remain upstream copies.

The upstream client decodes legacy WaveInfo and Wave as one combined buffer and
only dispatches Wave2 to the backend. An RDP server using pre-v8 audio (including
xrdp 0.9.24) sends these as two consecutive SVC messages, so the upstream first
message fails decoding before audio playback. This patch stores fixed-size
WaveInfo metadata, then accepts exactly one raw Wave continuation. Its four zero
padding bytes are replaced with the saved sample prefix. BodySize is a u16 and
must exceed 12; the resulting sample is bounded to 65527 bytes. No buffer is
allocated while waiting. A confirm is sent only after a full validated sample
has been delivered to the backend. The backend retains ownership of its bounded
playback queue and device errors.

Malformed messages, invalid format indices, an interrupted legacy pair (including
Close or format renegotiation between its two parts), or no common format stop
only this optional audio channel. Pending metadata is discarded and the backend
receives `protocol_error()`, a new default trait method delegating to `close()`.
NoriShell overrides it to flush audio and publish a non-secret failure state.
Normal Close and format renegotiation retain upstream behavior when no pair is
pending. A stopped channel does not resume on later messages; a new desktop
connection creates a fresh channel. Audio contents are omitted from channel
logging and pending-state Debug output.

Protocol references:
- [WaveInfo structure](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpea/c53cd81c-0d7f-4e68-8b95-1c1da68dbaac)
- [WaveInfo size and sample prefix](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpea/44d6fb1c-96c9-432c-b6d3-bdd3bfccff66)
- [Wave continuation padding and size](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpea/81841793-f5d3-4305-aa22-ddcbd81a96b5)

Validation target: `cargo test -p ironrdp-rdpsnd --lib`. Tests cover split
reassembly and confirmation, block-number wrapping, truncated/oversized/wrong
padding continuations, intervening control messages, invalid lengths/format
indices, maximum wire length, pending Debug redaction, normal close/renegotiation,
no common format, and existing Wave2 playback. Native server/device acceptance
is recorded in the project's implementation-status document, not inferred from
these tests.
