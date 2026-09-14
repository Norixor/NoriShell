// Modified for NoriShell; see vendor/README.md at the repository root for upstream provenance.
use core::mem;

use ironrdp_pdu::rdp;
use ironrdp_pdu::rdp::capability_sets::CapabilitySet;
use tracing::{debug, warn};

use crate::{
    Config, ConnectionFinalizationSequence, ConnectorError, ConnectorErrorExt as _, ConnectorResult, DesktopSize,
    Sequence, State, Written, general_err, reason_err,
};

/// Represents the Capability Exchange and Connection Finalization phases
/// of the connection sequence (section [1.3.1.1]).
///
/// This is abstracted into its own struct to allow it to be used for the ordinary
/// RDP connection sequence [`ClientConnector`] that occurs for every RDP connection,
/// as well as the Deactivation-Reactivation Sequence ([1.3.1.3]) that occurs when
/// a [Server Deactivate All PDU] is received.
///
/// [1.3.1.1]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/023f1e69-cfe8-4ee6-9ee0-7e759fb4e4ee
/// [1.3.1.3]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/dfc234ce-481a-4674-9a5d-2a7bafb14432
/// [`ClientConnector`]: crate::ClientConnector
/// [Server Deactivate All PDU]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/8a29971a-df3c-48da-add2-8ed9a05edc89
#[derive(Debug, Clone)]
pub struct ConnectionActivationSequence {
    state: ConnectionActivationState,
    config: Config,
    // The MCS channel IDs are invariant for the whole life of the sequence: they are negotiated
    // once and never change, even across a Deactivation-Reactivation Sequence. They are stored
    // here (rather than duplicated into every state variant).
    io_channel_id: u16,
    user_channel_id: u16,
}

impl ConnectionActivationSequence {
    pub fn new(config: Config, io_channel_id: u16, user_channel_id: u16) -> Self {
        // TODO/FIXME: Investigate whether we really need to carry around the whole `Config` struct.
        // RATIONALE(@CBenoit): Not very convenient when building in isolation.
        //   I doubt this type really needs every field there.
        Self {
            state: ConnectionActivationState::CapabilitiesExchange,
            config,
            io_channel_id,
            user_channel_id,
        }
    }

    pub fn io_channel_id(&self) -> u16 {
        self.io_channel_id
    }

    pub fn user_channel_id(&self) -> u16 {
        self.user_channel_id
    }

    /// Returns the current state as a distinct type, rather than `&dyn State` provided by [`Self::state`].
    pub fn connection_activation_state(&self) -> ConnectionActivationState {
        self.state
    }
}

/// Factory producing fresh [`ConnectionActivationSequence`] instances.
///
/// The [`Config`] and MCS channel IDs required to build a connection activation sequence are
/// invariant for the whole lifetime of the connection: they are negotiated once and never change,
/// even across a [Deactivation-Reactivation Sequence]. This factory captures them so that a fresh,
/// correctly-initialized sequence can be produced each time one is needed, driven until it is
/// finalized, then dropped.
///
/// [Deactivation-Reactivation Sequence]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/dfc234ce-481a-4674-9a5d-2a7bafb14432
#[derive(Debug, Clone)]
pub struct ConnectionActivationFactory {
    config: Config,
    io_channel_id: u16,
    user_channel_id: u16,
}

impl ConnectionActivationFactory {
    pub fn new(config: Config, io_channel_id: u16, user_channel_id: u16) -> Self {
        Self {
            config,
            io_channel_id,
            user_channel_id,
        }
    }

    pub fn io_channel_id(&self) -> u16 {
        self.io_channel_id
    }

    pub fn user_channel_id(&self) -> u16 {
        self.user_channel_id
    }

    /// Produces a fresh [`ConnectionActivationSequence`] in the initial `CapabilitiesExchange` state.
    #[must_use]
    pub fn create(&self) -> ConnectionActivationSequence {
        ConnectionActivationSequence::new(self.config.clone(), self.io_channel_id, self.user_channel_id)
    }
}

impl Sequence for ConnectionActivationSequence {
    fn next_pdu_hint(&self) -> Option<&dyn ironrdp_pdu::PduHint> {
        match &self.state {
            ConnectionActivationState::Consumed => None,
            ConnectionActivationState::Finalized { .. } => None,
            ConnectionActivationState::CapabilitiesExchange => Some(&ironrdp_pdu::X224_HINT),
            ConnectionActivationState::ConnectionFinalization {
                connection_finalization,
                ..
            } => connection_finalization.next_pdu_hint(),
        }
    }

    fn state(&self) -> &dyn State {
        &self.state
    }

    fn step(&mut self, input: &[u8], output: &mut ironrdp_core::WriteBuf) -> ConnectorResult<Written> {
        let (written, next_state) = match mem::take(&mut self.state) {
            ConnectionActivationState::Consumed | ConnectionActivationState::Finalized { .. } => {
                return Err(general_err!(
                    "connector sequence state is finalized or consumed (this is a bug)"
                ));
            }
            ConnectionActivationState::CapabilitiesExchange => {
                debug!("Capabilities Exchange");

                let send_data_indication_ctx =
                    ironrdp_pdu::mcs::decode_send_data_indication(input).map_err(ConnectorError::decode)?;
                let share_control_ctx =
                    rdp::headers::decode_share_control(send_data_indication_ctx).map_err(ConnectorError::decode)?;

                debug!(message = ?share_control_ctx.pdu, "Received");

                if share_control_ctx.channel_id != self.io_channel_id {
                    warn!(
                        io_channel_id = self.io_channel_id,
                        share_control_ctx.channel_id, "Unexpected channel ID for received Share Control Pdu"
                    );
                }

                // Some servers (e.g. GNOME Remote Desktop) send a ServerDeactivateAll PDU
                // before ServerDemandActive as part of a Deactivation-Reactivation Sequence
                // (MS-RDPBCGR §1.3.1.3). Skip it and stay in the same state to wait for
                // the actual DemandActive PDU.
                //
                // The decoded PDU is intentionally discarded: the DeactivateAll body carries
                // no payload we need during initial activation.
                if matches!(
                    share_control_ctx.pdu,
                    rdp::headers::ShareControlPdu::ServerDeactivateAll(_)
                ) {
                    debug!(
                        "Skipping Server Deactivate All PDU received during Capabilities Exchange, awaiting Server Demand Active"
                    );
                    self.state = ConnectionActivationState::CapabilitiesExchange;
                    return Ok(Written::Nothing);
                }

                let capability_sets = if let rdp::headers::ShareControlPdu::ServerDemandActive(server_demand_active) =
                    share_control_ctx.pdu
                {
                    server_demand_active.pdu.capability_sets
                } else {
                    return Err(reason_err!(
                        "ConnectionActivation::CapabilitiesExchange",
                        "unexpected Share Control PDU during capabilities exchange: got {} (expected Server Demand Active PDU)",
                        share_control_ctx.pdu.as_short_name(),
                    ));
                };

                for c in &capability_sets {
                    if let CapabilitySet::General(g) = c {
                        if g.protocol_version != rdp::capability_sets::PROTOCOL_VER {
                            warn!(version = g.protocol_version, "Unexpected protocol version");
                        }
                        break;
                    }
                }

                // At this point we have already sent a requested desktop size to the server -- either as a part of the
                // [`TS_UD_CS_CORE`] (on initial connection) or the [`DISPLAYCONTROL_MONITOR_LAYOUT`] (on resize event).
                //
                // The server is therefore responding with a desktop size here, which will be close to the requested size but
                // may be slightly different due to server-side constraints. We should use this negotiated size for the rest of
                // the session.
                //
                // [TS_UD_CS_CORE]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/00f1da4a-ee9c-421a-852f-c19f92343d73
                // [DISPLAYCONTROL_MONITOR_LAYOUT]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpedisp/ea2de591-9203-42cd-9908-be7a55237d1c
                let desktop_size = capability_sets
                    .iter()
                    .find_map(|c| match c {
                        CapabilitySet::Bitmap(b) => Some(DesktopSize {
                            width: b.desktop_width,
                            height: b.desktop_height,
                        }),
                        _ => None,
                    })
                    .unwrap_or(DesktopSize {
                        width: self.config.desktop_size.width,
                        height: self.config.desktop_size.height,
                    });

                let share_id = share_control_ctx.share_id;

                let client_confirm_active = rdp::headers::ShareControlPdu::ClientConfirmActive(
                    create_client_confirm_active(&self.config, capability_sets, desktop_size),
                );

                debug!(message = ?client_confirm_active, "Send");

                let written = rdp::headers::encode_share_control(
                    self.user_channel_id,
                    self.io_channel_id,
                    share_id,
                    client_confirm_active,
                    output,
                )
                .map_err(ConnectorError::encode)?;

                (
                    Written::from_size(written)?,
                    ConnectionActivationState::ConnectionFinalization {
                        desktop_size,
                        share_id,
                        connection_finalization: ConnectionFinalizationSequence::new(
                            self.io_channel_id,
                            self.user_channel_id,
                            share_id,
                        ),
                    },
                )
            }
            ConnectionActivationState::ConnectionFinalization {
                desktop_size,
                share_id,
                mut connection_finalization,
            } => {
                debug!("Connection Finalization");

                let written = connection_finalization.step(input, output)?;

                let next_state = if !connection_finalization.state.is_terminal() {
                    ConnectionActivationState::ConnectionFinalization {
                        desktop_size,
                        share_id,
                        connection_finalization,
                    }
                } else {
                    ConnectionActivationState::Finalized {
                        desktop_size,
                        share_id,
                        enable_server_pointer: self.config.enable_server_pointer,
                        pointer_software_rendering: self.config.pointer_software_rendering,
                    }
                };

                (written, next_state)
            }
        };

        self.state = next_state;

        Ok(written)
    }
}

#[derive(Default, Debug, Copy, Clone)]
pub enum ConnectionActivationState {
    #[default]
    Consumed,
    CapabilitiesExchange,
    ConnectionFinalization {
        desktop_size: DesktopSize,
        share_id: u32,
        connection_finalization: ConnectionFinalizationSequence,
    },
    Finalized {
        desktop_size: DesktopSize,
        share_id: u32,
        enable_server_pointer: bool,
        pointer_software_rendering: bool,
    },
}

impl State for ConnectionActivationState {
    fn name(&self) -> &'static str {
        match self {
            ConnectionActivationState::Consumed => "Consumed",
            ConnectionActivationState::CapabilitiesExchange => "CapabilitiesExchange",
            ConnectionActivationState::ConnectionFinalization { .. } => "ConnectionFinalization",
            ConnectionActivationState::Finalized { .. } => "Finalized",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, ConnectionActivationState::Finalized { .. })
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

const DEFAULT_POINTER_CACHE_SIZE: u16 = 32;

fn create_client_confirm_active(
    config: &Config,
    mut server_capability_sets: Vec<CapabilitySet>,
    desktop_size: DesktopSize,
) -> rdp::capability_sets::ClientConfirmActive {
    use ironrdp_pdu::rdp::capability_sets::{
        BITMAP_CACHE_ENTRIES_NUM, Bitmap, BitmapCache, BitmapDrawingFlags, Brush, CacheDefinition, CacheEntry,
        ClientConfirmActive, CmdFlags, DemandActive, FrameAcknowledge, GLYPH_CACHE_NUM, General, GeneralExtraFlags,
        GlyphCache, GlyphSupportLevel, Input, InputFlags, LargePointer, LargePointerSupportFlags, MultifragmentUpdate,
        OffscreenBitmapCache, Order, OrderFlags, OrderSupportExFlags, Pointer, SERVER_CHANNEL_ID, Sound, SoundFlags,
        SupportLevel, SurfaceCommands, VirtualChannel, VirtualChannelFlags, client_codecs_capabilities,
    };

    // Surface codecs use fast-path output. Negotiate against this activation's
    // Demand Active rather than advertising a transport the server disabled.
    // MS-RDPBCGR 2.2.7.1.1 / 2.2.7.2.9; MS-RDPRFX 1.5.
    let fastpath_output = server_capability_sets.iter().any(|capability| {
        matches!(capability, CapabilitySet::General(general)
            if general.extra_flags.contains(GeneralExtraFlags::FASTPATH_OUTPUT_SUPPORTED))
    });
    let surface_flags = if fastpath_output {
        server_capability_sets
            .iter()
            .find_map(|capability| match capability {
                CapabilitySet::SurfaceCommands(surface) => Some(surface.flags),
                _ => None,
            })
            .unwrap_or_else(CmdFlags::empty)
            & (CmdFlags::SET_SURFACE_BITS | CmdFlags::STREAM_SURFACE_BITS | CmdFlags::FRAME_MARKER)
    } else {
        CmdFlags::empty()
    };
    let frame_ack = surface_flags.contains(CmdFlags::FRAME_MARKER)
        && server_capability_sets
            .iter()
            .any(|capability| matches!(capability, CapabilitySet::FrameAcknowledge(_)));
    let mut codecs = config
        .bitmap
        .as_ref()
        .map(|bitmap| bitmap.codecs.clone())
        .unwrap_or_else(|| client_codecs_capabilities(&[]).expect("can't panic for &[]"));
    // RemoteFX requires Stream Surface Bits; other supported surface codecs
    // use Set Surface Bits. Bitmap-cache drawing orders are not advertised.
    codecs.0.retain(|codec| match &codec.property {
        rdp::capability_sets::CodecProperty::RemoteFx(_) => surface_flags.contains(CmdFlags::STREAM_SURFACE_BITS),
        _ => surface_flags.contains(CmdFlags::SET_SURFACE_BITS),
    });

    server_capability_sets.retain(|capability_set| matches!(capability_set, CapabilitySet::MultiFragmentUpdate(_)));

    let lossy_bitmap_compression = config
        .bitmap
        .as_ref()
        .map(|bitmap| bitmap.lossy_compression)
        .unwrap_or(false);

    let drawing_flags = if lossy_bitmap_compression {
        BitmapDrawingFlags::ALLOW_SKIP_ALPHA
            | BitmapDrawingFlags::ALLOW_DYNAMIC_COLOR_FIDELITY
            | BitmapDrawingFlags::ALLOW_COLOR_SUBSAMPLING
    } else {
        BitmapDrawingFlags::ALLOW_SKIP_ALPHA
    };

    server_capability_sets.extend_from_slice(&[
        CapabilitySet::General(General {
            major_platform_type: config.platform,
            extra_flags: GeneralExtraFlags::NO_BITMAP_COMPRESSION_HDR
                | if fastpath_output {
                    GeneralExtraFlags::FASTPATH_OUTPUT_SUPPORTED
                } else {
                    GeneralExtraFlags::empty()
                },
            ..Default::default()
        }),
        CapabilitySet::Bitmap(Bitmap {
            pref_bits_per_pix: 32,
            desktop_width: desktop_size.width,
            desktop_height: desktop_size.height,
            // This is required to be true in order for the Microsoft::Windows::RDS::DisplayControl DVC to work.
            desktop_resize_flag: true,
            drawing_flags,
        }),
        CapabilitySet::Order(Order::new(
            OrderFlags::NEGOTIATE_ORDER_SUPPORT | OrderFlags::ZERO_BOUNDS_DELTAS_SUPPORT,
            OrderSupportExFlags::empty(),
            0,
            0,
        )),
        CapabilitySet::BitmapCache(BitmapCache {
            caches: [CacheEntry {
                entries: 0,
                max_cell_size: 0,
            }; BITMAP_CACHE_ENTRIES_NUM],
        }),
        CapabilitySet::Input(Input {
            input_flags: InputFlags::all(),
            keyboard_layout: 0,
            keyboard_type: Some(config.keyboard_type),
            keyboard_subtype: config.keyboard_subtype,
            keyboard_function_key: config.keyboard_functional_keys_count,
            keyboard_ime_filename: config.ime_file_name.clone(),
        }),
        CapabilitySet::Pointer(Pointer {
            // Pointer cache should be set to non-zero value to enable client-side pointer rendering.
            color_pointer_cache_size: DEFAULT_POINTER_CACHE_SIZE,
            pointer_cache_size: DEFAULT_POINTER_CACHE_SIZE,
        }),
        CapabilitySet::Brush(Brush {
            support_level: SupportLevel::Default,
        }),
        CapabilitySet::GlyphCache(GlyphCache {
            glyph_cache: [CacheDefinition {
                entries: 0,
                max_cell_size: 0,
            }; GLYPH_CACHE_NUM],
            frag_cache: CacheDefinition {
                entries: 0,
                max_cell_size: 0,
            },
            glyph_support_level: GlyphSupportLevel::None,
        }),
        CapabilitySet::OffscreenBitmapCache(OffscreenBitmapCache {
            is_supported: false,
            cache_size: 0,
            cache_entries: 0,
        }),
        CapabilitySet::VirtualChannel(VirtualChannel {
            flags: VirtualChannelFlags::NO_COMPRESSION,
            chunk_size: Some(0), // ignored
        }),
        CapabilitySet::Sound(Sound {
            flags: SoundFlags::empty(),
        }),
        CapabilitySet::LargePointer(LargePointer {
            // Setting `LargePointerSupportFlags::UP_TO_384X384_PIXELS` allows server to send
            // `TS_FP_LARGEPOINTERATTRIBUTE` update messages, which are required for client-side
            // rendering of pointers bigger than 96x96 pixels.
            // `LargePointerSupportFlags::UP_TO_96X96_PIXELS` is needed for proper cursor behavior
            // in Windows 2019 and older
            flags: LargePointerSupportFlags::UP_TO_96X96_PIXELS | LargePointerSupportFlags::UP_TO_384X384_PIXELS,
        }),
    ]);

    if !surface_flags.is_empty() {
        server_capability_sets.push(CapabilitySet::SurfaceCommands(SurfaceCommands { flags: surface_flags }));
    }
    if !codecs.0.is_empty() {
        server_capability_sets.push(CapabilitySet::BitmapCodecs(codecs));
    }
    if frame_ack {
        server_capability_sets.push(CapabilitySet::FrameAcknowledge(FrameAcknowledge {
            // Preserve the upstream resize workaround (IronRDP issue #447).
            max_unacknowledged_frame_count: 20,
        }));
    }

    if !server_capability_sets
        .iter()
        .any(|c| matches!(&c, CapabilitySet::MultiFragmentUpdate(_)))
    {
        server_capability_sets.push(CapabilitySet::MultiFragmentUpdate(MultifragmentUpdate {
            max_request_size: 8 * 1024 * 1024, // 8 MB
        }));
    }

    ClientConfirmActive {
        originator_id: SERVER_CHANNEL_ID,
        pdu: DemandActive {
            source_descriptor: "IRONRDP".to_owned(),
            capability_sets: server_capability_sets,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_core::{WriteBuf, decode, encode_vec};
    use ironrdp_pdu::{gcc::KeyboardType, mcs, x224::X224};
    use rdp::{
        capability_sets::*,
        client_info::{PerformanceFlags, TimezoneInfo},
        headers::*,
    };

    fn config() -> Config {
        Config {
            credentials: crate::Credentials::UsernamePassword {
                username: "qa".into(),
                password: String::new(),
            },
            domain: None,
            enable_tls: true,
            enable_credssp: true,
            keyboard_type: KeyboardType::IbmEnhanced,
            keyboard_subtype: 0,
            keyboard_layout: 0,
            keyboard_functional_keys_count: 12,
            ime_file_name: String::new(),
            dig_product_id: String::new(),
            desktop_size: DesktopSize {
                width: 1024,
                height: 768,
            },
            bitmap: None,
            client_build: 0,
            client_name: "NoriShell".into(),
            client_dir: "C:\\Windows\\System32\\mstscax.dll".into(),
            platform: MajorPlatformType::UNSPECIFIED,
            enable_server_pointer: true,
            request_data: None,
            autologon: true,
            enable_audio_playback: false,
            compression_type: None,
            pointer_software_rendering: true,
            multitransport_flags: None,
            performance_flags: PerformanceFlags::default(),
            desktop_scale_factor: 0,
            hardware_id: None,
            license_cache: None,
            timezone_info: TimezoneInfo::default(),
            alternate_shell: String::new(),
            work_dir: String::new(),
        }
    }

    fn offered(fastpath: bool, flags: Option<CmdFlags>, ack: bool) -> Vec<CapabilitySet> {
        let mut caps = vec![CapabilitySet::General(General {
            extra_flags: if fastpath {
                GeneralExtraFlags::FASTPATH_OUTPUT_SUPPORTED
            } else {
                GeneralExtraFlags::empty()
            },
            ..Default::default()
        })];
        if let Some(flags) = flags {
            caps.push(CapabilitySet::SurfaceCommands(SurfaceCommands { flags }));
        }
        if ack {
            caps.push(CapabilitySet::FrameAcknowledge(FrameAcknowledge {
                max_unacknowledged_frame_count: 2,
            }));
        }
        caps
    }

    // Drive the real capability exchange used both by initial activation and
    // ConnectionActivationFactory after Server Deactivate All (e.g. resize).
    fn exchange(sequence: &mut ConnectionActivationSequence, caps: Vec<CapabilitySet>) -> Vec<CapabilitySet> {
        let header = ShareControlHeader {
            share_control_pdu: ShareControlPdu::ServerDemandActive(ServerDemandActive {
                pdu: DemandActive {
                    source_descriptor: "QA".into(),
                    capability_sets: caps,
                },
            }),
            pdu_source: 1002,
            share_id: 1,
        };
        let packet = encode_vec(&X224(mcs::SendDataIndication {
            initiator_id: 1002,
            channel_id: 1003,
            user_data: encode_vec(&header).unwrap().into(),
        }))
        .unwrap();
        let mut output = WriteBuf::new();
        sequence.step(&packet, &mut output).unwrap();
        let response: X224<mcs::SendDataRequest<'_>> = decode(output.filled()).unwrap();
        let header: ShareControlHeader = decode(&response.0.user_data).unwrap();
        let ShareControlPdu::ClientConfirmActive(confirm) = header.share_control_pdu else {
            panic!("expected Confirm Active")
        };
        confirm.pdu.capability_sets
    }

    fn has_rfx(caps: &[CapabilitySet]) -> bool {
        caps.iter().any(|cap| {
            matches!(cap, CapabilitySet::BitmapCodecs(codecs)
            if codecs.0.iter().any(|codec| matches!(codec.property, CodecProperty::RemoteFx(_))))
        })
    }

    #[test]
    fn initial_and_reactivation_recompute_surface_capabilities() {
        let all = CmdFlags::SET_SURFACE_BITS | CmdFlags::STREAM_SURFACE_BITS | CmdFlags::FRAME_MARKER;
        let factory = ConnectionActivationFactory::new(config(), 1003, 1002);
        let mut initial = ConnectionActivationSequence::new(config(), 1003, 1002);
        let caps = exchange(&mut initial, offered(true, Some(all), true));
        assert!(has_rfx(&caps));
        assert!(
            caps.iter()
                .any(|c| matches!(c, CapabilitySet::SurfaceCommands(s) if s.flags == all))
        );
        assert!(caps.iter().any(|c| matches!(c, CapabilitySet::FrameAcknowledge(_))));
        // A later activation can withdraw support; the prior decision must not leak.
        for offered in [offered(false, Some(all), true), offered(true, None, true), Vec::new()] {
            let caps = exchange(&mut factory.create(), offered);
            assert!(!has_rfx(&caps));
            assert!(!caps.iter().any(|c| matches!(
                c,
                CapabilitySet::SurfaceCommands(_) | CapabilitySet::FrameAcknowledge(_) | CapabilitySet::BitmapCodecs(_)
            )));
            assert!(caps.iter().any(|c| matches!(c, CapabilitySet::Bitmap(_))));
        }
        assert!(has_rfx(&exchange(
            &mut factory.create(),
            offered(true, Some(all), true)
        )));
    }

    #[test]
    fn partial_surface_flags_do_not_overadvertise_rfx_or_frame_ack() {
        let factory = ConnectionActivationFactory::new(config(), 1003, 1002);
        let caps = exchange(
            &mut factory.create(),
            offered(true, Some(CmdFlags::SET_SURFACE_BITS), true),
        );
        assert!(!has_rfx(&caps));
        assert!(!caps.iter().any(|c| matches!(c, CapabilitySet::FrameAcknowledge(_))));
        assert!(
            caps.iter()
                .any(|c| matches!(c, CapabilitySet::SurfaceCommands(s) if s.flags == CmdFlags::SET_SURFACE_BITS))
        );
        let caps = exchange(
            &mut factory.create(),
            offered(
                true,
                Some(CmdFlags::STREAM_SURFACE_BITS | CmdFlags::FRAME_MARKER),
                false,
            ),
        );
        assert!(has_rfx(&caps));
        assert!(!caps.iter().any(|c| matches!(c, CapabilitySet::FrameAcknowledge(_))));
    }
}
