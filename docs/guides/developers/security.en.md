# Security and release checklist

Design a plugin as an untrusted proposer. Bind every result to the request and resource handle Core supplied; do not persist handles as authority, retry an `outcomeUnknown` result automatically, or recreate a resource after cancellation. Validate inputs and bound rendering before sending a document.

Before shipping, verify a supported manifest minor (baseline 13 through host minor within major 1) and matching resident SDK ABI; denied and withdrawn grants; stale target/generation rejection; resource close and event backpressure; storage conflicts; exact-origin credential behavior; and no secret in output, state, storage, audit text, or errors. Validate actual macOS and Windows Tauri lifecycle separately when the relevant capability requires it.
