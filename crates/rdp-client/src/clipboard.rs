//! Explicitly enabled text exchange only; never reads the system clipboard or redirects files.
use ironrdp::{
    cliprdr::{backend::CliprdrBackend, pdu::*},
    core::impl_as_any,
};
use norishell_desktop_protocol::{EngineEvent, EventSink, MAX_TEXT_BYTES};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
#[derive(Default)]
pub(crate) struct State {
    pub ready: bool,
    pub text: Option<String>,
    pub advertise: bool,
    pub request: bool,
    pub response: Option<OwnedFormatDataResponse>,
}
pub(crate) struct Backend {
    pub state: Arc<Mutex<State>>,
    pub events: EventSink,
}
impl fmt::Debug for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TextClipboardBackend")
    }
}
impl_as_any!(Backend);
impl CliprdrBackend for Backend {
    fn temporary_directory(&self) -> &str {
        ""
    }
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }
    fn on_ready(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.ready = true;
        }
    }
    fn on_request_format_list(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.advertise = true;
        }
    }
    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}
    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        if let Ok(mut state) = self.state.lock() {
            state.request = formats
                .iter()
                .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT);
        }
    }
    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        if let Ok(mut state) = self.state.lock() {
            state.response = Some(if request.format == ClipboardFormatId::CF_UNICODETEXT {
                state
                    .text
                    .as_deref()
                    .map(FormatDataResponse::new_unicode_string)
                    .unwrap_or_else(FormatDataResponse::new_error)
            } else {
                FormatDataResponse::new_error()
            });
        }
    }
    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if !response.is_error()
            && response.data().len() <= MAX_TEXT_BYTES * 2 + 2
            && let Ok(text) = response.to_unicode_string()
            && text.len() <= MAX_TEXT_BYTES
            && !text.contains('\0')
        {
            (self.events)(EngineEvent::Clipboard(text));
        }
    }
    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _: LockDataId) {}
    fn on_unlock(&mut self, _: LockDataId) {}
}
