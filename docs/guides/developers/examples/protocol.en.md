# Protocol Demo: a framed TCP terminal provider

Protocol Demo is for authors building a protocol provider, not an ordinary panel. It declares a fixed `framedTcp` provider in `assets/protocols.json` and requests `uiPanel`, `terminalProvider`, and `networkDomain`.

The visible **Open framed TCP terminal** action emits one typed `protocolOpen` request. It does not create a socket itself. Once Core starts the claimed terminal, the guest receives provider events. The sequence below is an explanation of the real provider lifecycle, not standalone guest code:

```text
Connect -> networkStart(tcp endpoint)
Opened -> Ready
Input -> networkSend(frame type 0x01)
Resize -> networkSend(frame type 0x02)
Data -> terminal bytes from frame type 0x81
Close -> discard only that connection's guest state
```

Frames are four-byte big-endian `(type + payload)` length, followed by the type byte and opaque payload. The guest keeps a separate partial-frame buffer for each `connectionHandle`; it does not reinterpret UTF-8 or ANSI data. Core owns the TCP resource and closes it before notifying the guest, so a Close event cannot be used to regain a broker call.

| If you see… | Input and expected result |
| --- | --- |
| No terminal opens | Start from the visible action and provide the provider's configured TCP endpoint through the Core flow; a UI action alone must only emit `protocolOpen`. |
| Input or resize is ignored | After Core reports `Opened`, expect type `0x01` for input and `0x02` for resize through typed `networkSend` requests. |
| Terminal bytes look corrupt | Preserve each handle's partial-frame buffer and forward only type `0x81` payload bytes without UTF-8/ANSI conversion. |
| One connection closes another | Verify state is keyed by `connectionHandle`; Close discards only the matching guest state. |

Build the reproducible local candidate from the NoriShell checkout root:

```sh
python3 examples/plugins/protocol-demo/build.py \
  --output /tmp/NoriShell-Framed-TCP-Protocol-Demo-1.0.3.zip
```

The script runs a loopback fixture through the production persistent Wasm runtime, checks input, resize, split ANSI/UTF-8 byte frames, two independent handles, and old-handle close isolation, then validates the ZIP. It does not install the ZIP or start the desktop application. Desktop terminal lifecycle and protected approval remain separate acceptance work. Read [calling Core](../development/calling-api.en.md), [errors](../development/errors.en.md), and [packaging](../development/packaging.en.md) before adapting it.
