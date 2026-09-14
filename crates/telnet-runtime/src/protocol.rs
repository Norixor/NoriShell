use thiserror::Error;

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_BINARY: u8 = 0;
const OPT_ECHO: u8 = 1;
const OPT_SUPPRESS_GO_AHEAD: u8 = 3;
const OPT_TERMINAL_TYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;
const MAX_SUBNEGOTIATION_BYTES: usize = 4 * 1024;
const DEFAULT_TERMINAL_TYPE: &[u8] = b"xterm-256color";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegotiatedOption {
    LocalNaws,
    LocalTerminalType,
    LocalBinary,
    RemoteBinary,
    RemoteEcho,
    RemoteSuppressGoAhead,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DecodeResult {
    pub data: Vec<u8>,
    pub responses: Vec<u8>,
    pub enabled: Vec<NegotiatedOption>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("Telnet subnegotiation exceeded the bounded protocol limit")]
    SubnegotiationTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Data,
    Iac,
    Negotiation(u8),
    SubnegotiationOption,
    Subnegotiation,
    SubnegotiationIac,
}

/// Stateful, fragmentation-safe Telnet decoder with an explicit option
/// allowlist. Unknown options are refused once and never enter terminal data.
pub struct TelnetCodec {
    state: ParseState,
    subnegotiation_option: Option<u8>,
    subnegotiation: Vec<u8>,
    local_enabled: [bool; 256],
    remote_enabled: [bool; 256],
    local_refused: [bool; 256],
    remote_refused: [bool; 256],
}

impl Default for TelnetCodec {
    fn default() -> Self {
        Self {
            state: ParseState::Data,
            subnegotiation_option: None,
            subnegotiation: Vec::new(),
            local_enabled: [false; 256],
            remote_enabled: [false; 256],
            local_refused: [false; 256],
            remote_refused: [false; 256],
        }
    }
}

impl TelnetCodec {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn local_naws_enabled(&self) -> bool {
        self.local_enabled[usize::from(OPT_NAWS)]
    }

    pub fn decode(&mut self, bytes: &[u8]) -> Result<DecodeResult, ProtocolError> {
        let mut decoded = DecodeResult::default();
        for &byte in bytes {
            match self.state {
                ParseState::Data => {
                    if byte == IAC {
                        self.state = ParseState::Iac;
                    } else {
                        decoded.data.push(byte);
                    }
                }
                ParseState::Iac => match byte {
                    IAC => {
                        decoded.data.push(IAC);
                        self.state = ParseState::Data;
                    }
                    DO | DONT | WILL | WONT => self.state = ParseState::Negotiation(byte),
                    SB => self.state = ParseState::SubnegotiationOption,
                    _ => self.state = ParseState::Data,
                },
                ParseState::Negotiation(command) => {
                    self.handle_negotiation(command, byte, &mut decoded);
                    self.state = ParseState::Data;
                }
                ParseState::SubnegotiationOption => {
                    self.subnegotiation_option = Some(byte);
                    self.subnegotiation.clear();
                    self.state = ParseState::Subnegotiation;
                }
                ParseState::Subnegotiation => {
                    if byte == IAC {
                        self.state = ParseState::SubnegotiationIac;
                    } else {
                        self.push_subnegotiation(byte)?;
                    }
                }
                ParseState::SubnegotiationIac => {
                    if byte == IAC {
                        self.push_subnegotiation(IAC)?;
                        self.state = ParseState::Subnegotiation;
                    } else if byte == SE {
                        self.finish_subnegotiation(&mut decoded);
                        self.state = ParseState::Data;
                    } else {
                        // A malformed command terminates this untrusted SB
                        // block. It is discarded rather than exposed as data.
                        self.subnegotiation.clear();
                        self.subnegotiation_option = None;
                        self.state = ParseState::Data;
                    }
                }
            }
        }
        Ok(decoded)
    }

    fn push_subnegotiation(&mut self, byte: u8) -> Result<(), ProtocolError> {
        if self.subnegotiation.len() >= MAX_SUBNEGOTIATION_BYTES {
            self.subnegotiation.clear();
            self.subnegotiation_option = None;
            self.state = ParseState::Data;
            return Err(ProtocolError::SubnegotiationTooLarge);
        }
        self.subnegotiation.push(byte);
        Ok(())
    }

    fn handle_negotiation(&mut self, command: u8, option: u8, decoded: &mut DecodeResult) {
        let option_index = usize::from(option);
        match command {
            DO if allows_local(option) => {
                if !self.local_enabled[option_index] {
                    self.local_enabled[option_index] = true;
                    self.local_refused[option_index] = false;
                    decoded.responses.extend_from_slice(&[IAC, WILL, option]);
                    decoded.enabled.push(local_option(option));
                }
            }
            DO => {
                if !self.local_refused[option_index] {
                    self.local_refused[option_index] = true;
                    decoded.responses.extend_from_slice(&[IAC, WONT, option]);
                }
            }
            DONT => {
                if self.local_enabled[option_index] {
                    self.local_enabled[option_index] = false;
                    decoded.responses.extend_from_slice(&[IAC, WONT, option]);
                }
            }
            WILL if allows_remote(option) => {
                if !self.remote_enabled[option_index] {
                    self.remote_enabled[option_index] = true;
                    self.remote_refused[option_index] = false;
                    decoded.responses.extend_from_slice(&[IAC, DO, option]);
                    decoded.enabled.push(remote_option(option));
                }
            }
            WILL => {
                if !self.remote_refused[option_index] {
                    self.remote_refused[option_index] = true;
                    decoded.responses.extend_from_slice(&[IAC, DONT, option]);
                }
            }
            WONT if self.remote_enabled[option_index] => {
                self.remote_enabled[option_index] = false;
                decoded.responses.extend_from_slice(&[IAC, DONT, option]);
            }
            _ => {}
        }
    }

    fn finish_subnegotiation(&mut self, decoded: &mut DecodeResult) {
        if self.subnegotiation_option == Some(OPT_TERMINAL_TYPE)
            && self.local_enabled[usize::from(OPT_TERMINAL_TYPE)]
            && self.subnegotiation.as_slice() == [TTYPE_SEND]
        {
            decoded
                .responses
                .extend_from_slice(&[IAC, SB, OPT_TERMINAL_TYPE, TTYPE_IS]);
            append_escaped(&mut decoded.responses, DEFAULT_TERMINAL_TYPE);
            decoded.responses.extend_from_slice(&[IAC, SE]);
        }
        self.subnegotiation.clear();
        self.subnegotiation_option = None;
    }
}

fn allows_local(option: u8) -> bool {
    matches!(option, OPT_BINARY | OPT_TERMINAL_TYPE | OPT_NAWS)
}

fn allows_remote(option: u8) -> bool {
    matches!(option, OPT_BINARY | OPT_ECHO | OPT_SUPPRESS_GO_AHEAD)
}

fn local_option(option: u8) -> NegotiatedOption {
    match option {
        OPT_NAWS => NegotiatedOption::LocalNaws,
        OPT_TERMINAL_TYPE => NegotiatedOption::LocalTerminalType,
        OPT_BINARY => NegotiatedOption::LocalBinary,
        _ => unreachable!("only allowlisted local options reach this function"),
    }
}

fn remote_option(option: u8) -> NegotiatedOption {
    match option {
        OPT_BINARY => NegotiatedOption::RemoteBinary,
        OPT_ECHO => NegotiatedOption::RemoteEcho,
        OPT_SUPPRESS_GO_AHEAD => NegotiatedOption::RemoteSuppressGoAhead,
        _ => unreachable!("only allowlisted remote options reach this function"),
    }
}

#[must_use]
pub fn encode_user_input(data: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(data.len());
    append_escaped(&mut encoded, data);
    encoded
}

#[must_use]
pub fn encode_naws(rows: u16, cols: u16) -> Vec<u8> {
    let mut encoded = vec![IAC, SB, OPT_NAWS];
    append_escaped(&mut encoded, &cols.to_be_bytes());
    append_escaped(&mut encoded, &rows.to_be_bytes());
    encoded.extend_from_slice(&[IAC, SE]);
    encoded
}

fn append_escaped(target: &mut Vec<u8>, bytes: &[u8]) {
    for &byte in bytes {
        target.push(byte);
        if byte == IAC {
            target.push(IAC);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_fragmented_raw_data_and_unescapes_iac() {
        let mut codec = TelnetCodec::new();
        let first = codec.decode(&[b'a', IAC]).expect("first fragment");
        let second = codec.decode(&[IAC, 0, 0xfe]).expect("second fragment");

        assert_eq!(first.data, b"a");
        assert_eq!(second.data, [IAC, 0, 0xfe]);
    }

    #[test]
    fn negotiates_only_allowlisted_options_and_suppresses_repeat_loops() {
        let mut codec = TelnetCodec::new();
        let decoded = codec
            .decode(&[
                IAC, DO, OPT_NAWS, IAC, DO, OPT_NAWS, IAC, DO, 99, IAC, DO, 99, IAC, WILL,
                OPT_ECHO, IAC, WILL, 98,
            ])
            .expect("negotiation");

        assert_eq!(
            decoded.responses,
            [
                IAC, WILL, OPT_NAWS, IAC, WONT, 99, IAC, DO, OPT_ECHO, IAC, DONT, 98,
            ]
        );
        assert_eq!(
            decoded.enabled,
            [NegotiatedOption::LocalNaws, NegotiatedOption::RemoteEcho]
        );
    }

    #[test]
    fn responds_to_terminal_type_send_after_negotiation() {
        let mut codec = TelnetCodec::new();
        codec
            .decode(&[IAC, DO, OPT_TERMINAL_TYPE])
            .expect("enable terminal type");
        let decoded = codec
            .decode(&[IAC, SB, OPT_TERMINAL_TYPE, TTYPE_SEND, IAC, SE])
            .expect("terminal type request");

        let mut expected = vec![IAC, SB, OPT_TERMINAL_TYPE, TTYPE_IS];
        expected.extend_from_slice(DEFAULT_TERMINAL_TYPE);
        expected.extend_from_slice(&[IAC, SE]);
        assert_eq!(decoded.responses, expected);
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn bounds_untrusted_subnegotiation() {
        let mut codec = TelnetCodec::new();
        let mut input = vec![IAC, SB, OPT_TERMINAL_TYPE];
        input.extend(std::iter::repeat_n(b'x', MAX_SUBNEGOTIATION_BYTES + 1));

        assert_eq!(
            codec.decode(&input),
            Err(ProtocolError::SubnegotiationTooLarge)
        );
    }

    #[test]
    fn escapes_user_iac_and_naws_payload_bytes() {
        assert_eq!(encode_user_input(&[1, IAC, 2]), [1, IAC, IAC, 2]);
        assert_eq!(
            encode_naws(24, 255),
            [IAC, SB, OPT_NAWS, 0, IAC, IAC, 0, 24, IAC, SE]
        );
    }
}
