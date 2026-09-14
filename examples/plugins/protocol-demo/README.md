# NoriShell Framed TCP protocol demo

This is a real protocol-13 Rust/Wasm provider package. It declares the fixed
`framedTcp` provider through `assets/protocols.json`; the catalog enables only
the TCP resource and resize support. The guest provides a visible **Open framed
TCP terminal** action, which emits one typed `ProtocolOpen` request. It does not
create a socket from its UI action.

When Core starts the claimed terminal, the guest handles the provider event
sequence. `Connect` emits a typed `NetworkStart` for the configured `tcp://`
endpoint. Core remains the TCP owner and sends resource events back into the
same Wasm instance. The first `Opened` event yields `Ready`; Input and Resize
become type `0x01` and `0x02` frames through `NetworkSend`. Inbound `Data`
events retain a separate partial-frame buffer for every `connectionHandle` and
produce raw terminal bytes only from type `0x81` output frames. The framing is
`4-byte big-endian (type + payload)`, `type`, then opaque payload. No UTF-8 or
ANSI transformation occurs in the plugin.

`Close` drops only the matching Wasm connection state. Core owns resource
shutdown, so a Close event cannot use it to obtain another broker call. This
mirrors the production driver, which closes Core resources before notifying the
guest.

Build a reproducible local candidate from the repository root:

```sh
python3 examples/plugins/protocol-demo/build.py \
  --output /tmp/NoriShell-Framed-TCP-Protocol-Demo-1.0.3.zip
```

The script uses `CARGO_INCREMENTAL=0`, builds the guest for
`wasm32-unknown-unknown`, starts `fixture_server.py` on a loopback ephemeral
port, and executes the generated Wasm with the production persistent
`WasmRuntime`. Before exercising the protocol, the native harness sends the
real `Initialize` request, parses the typed contribution, and passes its dialog
document through Core's production UI validator. It revalidates the declared
open action before dispatching it, checks the emitted typed broker call uses
the Core-compatible `protocol-demo.open` call ID, and validates the replacement
dialog emitted after the typed `BrokerResult`. Version 1.0.3 uses the
host-supported `link` icon for the open action. The harness then acts only as
the test Core broker after it observes typed `NetworkStart` and `NetworkSend`
outputs. It verifies actual TCP bytes for Input and Resize, split resource Data
for an ANSI/UTF-8 frame, two independent connection handles, and that closing
the old handle leaves the new connection usable. It then writes the versioned
ZIP and its adjacent `.sha256` file. It neither installs the ZIP nor starts the
desktop app; real Tauri and approval-surface acceptance remains an integration
responsibility.
