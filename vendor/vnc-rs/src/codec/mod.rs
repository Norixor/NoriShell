// Modified for NoriShell; see vendor/README.md at the repository root for upstream provenance.
mod raw;
mod tight;
mod zlib;
mod zrle;
pub(crate) use raw::Decoder as RawDecoder;
pub(crate) use tight::Decoder as TightDecoder;
pub(crate) use zrle::Decoder as ZrleDecoder;

fn uninit_vec(len: usize) -> Vec<u8> {
    // Decoder callers validate every rectangle and compressed payload before
    // allocating. Initializing keeps malformed inputs from ever exposing
    // uninitialized bytes if a read fails part way through.
    vec![0; len]
}
