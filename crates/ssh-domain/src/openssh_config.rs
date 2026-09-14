//! Bounded, side-effect-free preview parsing for the non-secret OpenSSH config subset.
//!
//! This module deliberately does not resolve `Include`, expand environment variables,
//! execute commands, or read identity files. Unsupported semantics are retained as
//! blocking diagnostics so callers cannot mistake a partial preview for an importable
//! host.

use std::collections::HashSet;

use crate::Endpoint;

pub const MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub const MAX_CONFIG_LINES: usize = 8_192;
pub const MAX_LINE_BYTES: usize = 4_096;
pub const MAX_CANDIDATES: usize = 512;
pub const MAX_IDENTITY_FILE_HINTS: usize = 64;
pub const MAX_JUMP_HOPS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenSshDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenSshDiagnosticCode {
    InputTooLarge,
    TooManyLines,
    LineTooLong,
    TooManyCandidates,
    TooManyIdentityFiles,
    InvalidSyntax,
    InvalidHostAlias,
    InvalidHostName,
    InvalidUser,
    InvalidPort,
    InvalidIdentityFile,
    InvalidProxyJump,
    UnsupportedProxyCommand,
    UnsupportedDirective,
    UnsupportedHostPattern,
    IncludeUnsupported,
    MatchUnsupported,
    DangerousExpansion,
    DangerousToken,
    NoLiteralHostCandidates,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshDiagnostic {
    pub code: OpenSshDiagnosticCode,
    pub severity: OpenSshDiagnosticSeverity,
    pub line: Option<usize>,
    pub directive: Option<String>,
    pub message: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshEndpointPreview {
    pub address: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshJumpHopPreview {
    pub endpoint: OpenSshEndpointPreview,
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenSshRoutePreview {
    Direct,
    JumpChain(Vec<OpenSshJumpHopPreview>),
    HttpConnect { proxy: OpenSshEndpointPreview },
    Socks5 { proxy: OpenSshEndpointPreview },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshHostCandidate {
    pub alias: String,
    pub endpoint: Option<OpenSshEndpointPreview>,
    pub user: Option<String>,
    pub identity_file_hints: Vec<String>,
    pub route: OpenSshRoutePreview,
    pub diagnostics: Vec<OpenSshDiagnostic>,
    pub importable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshConfigPreview {
    pub candidates: Vec<OpenSshHostCandidate>,
    pub diagnostics: Vec<OpenSshDiagnostic>,
}

#[derive(Debug, Clone)]
enum BlockSelector {
    Global,
    Host {
        patterns: Vec<HostPattern>,
        ambiguous: bool,
    },
    Match,
}

impl BlockSelector {
    fn matches(&self, alias: &str) -> bool {
        match self {
            Self::Global | Self::Match => true,
            Self::Host {
                patterns,
                ambiguous,
            } => {
                if *ambiguous {
                    return true;
                }
                let mut positive_match = false;
                for pattern in patterns {
                    if glob_matches(&pattern.pattern, alias) {
                        if pattern.negated {
                            return false;
                        }
                        positive_match = true;
                    }
                }
                positive_match
            }
        }
    }
}

#[derive(Debug, Clone)]
struct HostPattern {
    pattern: String,
    negated: bool,
}

#[derive(Debug, Clone)]
struct ConfigBlock {
    selector: BlockSelector,
    directives: Vec<ParsedDirective>,
    diagnostics: Vec<OpenSshDiagnostic>,
}

impl ConfigBlock {
    fn global() -> Self {
        Self {
            selector: BlockSelector::Global,
            directives: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
enum ParsedDirective {
    HostName {
        value: String,
        line: usize,
    },
    User {
        value: String,
        line: usize,
    },
    Port {
        value: String,
        line: usize,
    },
    IdentityFile {
        value: String,
        line: usize,
    },
    Route {
        value: RouteSource,
        line: usize,
    },
    Blocking {
        diagnostic: OpenSshDiagnostic,
        slot: DirectiveSlot,
    },
    Ignored,
}

#[derive(Debug, Clone, Copy)]
enum DirectiveSlot {
    Always,
    HostName,
    User,
    Port,
    IdentityFile,
    Route,
}

#[derive(Debug, Clone)]
enum RouteSource {
    ProxyJump(String),
    ProxyCommand(Vec<String>),
}

#[derive(Debug)]
struct ParsedLine {
    keyword: String,
    values: Vec<String>,
}

/// Parses an OpenSSH config string into bounded, non-secret import previews.
///
/// The parser has no filesystem, process, network, or environment side effects.
#[must_use]
pub fn parse_openssh_config_preview(input: &str) -> OpenSshConfigPreview {
    if input.len() > MAX_CONFIG_BYTES {
        return OpenSshConfigPreview {
            candidates: Vec::new(),
            diagnostics: vec![diagnostic(
                OpenSshDiagnosticCode::InputTooLarge,
                None,
                None,
                "OpenSSH config exceeds the preview byte limit",
                true,
            )],
        };
    }

    let mut blocks = Vec::new();
    let mut current = ConfigBlock::global();
    let mut aliases = Vec::new();
    let mut seen_aliases = HashSet::new();
    let mut document_diagnostics = Vec::new();
    let mut candidate_limit_reported = false;

    for (index, raw_line) in input.lines().enumerate() {
        let line_number = index + 1;
        if index >= MAX_CONFIG_LINES {
            document_diagnostics.push(diagnostic(
                OpenSshDiagnosticCode::TooManyLines,
                Some(line_number),
                None,
                "OpenSSH config exceeds the preview line limit",
                true,
            ));
            break;
        }
        if raw_line.len() > MAX_LINE_BYTES {
            document_diagnostics.push(diagnostic(
                OpenSshDiagnosticCode::LineTooLong,
                Some(line_number),
                None,
                "OpenSSH config line exceeds the preview line-length limit",
                true,
            ));
            continue;
        }

        let parsed = match parse_line(raw_line) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => continue,
            Err(message) => {
                current.diagnostics.push(diagnostic(
                    OpenSshDiagnosticCode::InvalidSyntax,
                    Some(line_number),
                    None,
                    &message,
                    true,
                ));
                continue;
            }
        };

        if parsed.keyword.eq_ignore_ascii_case("host") {
            blocks.push(current);
            let (selector, discovered, diagnostics) =
                parse_host_selector(&parsed.values, line_number);
            current = ConfigBlock {
                selector,
                directives: Vec::new(),
                diagnostics,
            };
            for alias in discovered {
                let key = alias.to_ascii_lowercase();
                if seen_aliases.contains(&key) {
                    continue;
                }
                if aliases.len() == MAX_CANDIDATES {
                    if !candidate_limit_reported {
                        document_diagnostics.push(diagnostic(
                            OpenSshDiagnosticCode::TooManyCandidates,
                            Some(line_number),
                            Some("Host"),
                            "OpenSSH config exceeds the literal Host candidate limit",
                            true,
                        ));
                        candidate_limit_reported = true;
                    }
                    continue;
                }
                seen_aliases.insert(key);
                aliases.push(alias);
            }
            continue;
        }

        if parsed.keyword.eq_ignore_ascii_case("match") {
            blocks.push(current);
            let code = if parsed
                .values
                .iter()
                .any(|value| value.eq_ignore_ascii_case("exec"))
            {
                OpenSshDiagnosticCode::DangerousToken
            } else {
                OpenSshDiagnosticCode::MatchUnsupported
            };
            current = ConfigBlock {
                selector: BlockSelector::Match,
                directives: Vec::new(),
                diagnostics: vec![diagnostic(
                    code,
                    Some(line_number),
                    Some("Match"),
                    "Match blocks are not evaluated by the safe import preview",
                    true,
                )],
            };
            continue;
        }

        current
            .directives
            .push(parse_directive(parsed, line_number));
    }
    blocks.push(current);

    if aliases.is_empty() {
        document_diagnostics.push(diagnostic(
            OpenSshDiagnosticCode::NoLiteralHostCandidates,
            None,
            Some("Host"),
            "No literal Host aliases were found",
            false,
        ));
    }

    let candidates = aliases
        .into_iter()
        .map(|alias| build_candidate(&alias, &blocks, &document_diagnostics))
        .collect();
    OpenSshConfigPreview {
        candidates,
        diagnostics: document_diagnostics,
    }
}

fn parse_host_selector(
    values: &[String],
    line: usize,
) -> (BlockSelector, Vec<String>, Vec<OpenSshDiagnostic>) {
    if values.is_empty() {
        return (
            BlockSelector::Host {
                patterns: Vec::new(),
                ambiguous: true,
            },
            Vec::new(),
            vec![diagnostic(
                OpenSshDiagnosticCode::InvalidSyntax,
                Some(line),
                Some("Host"),
                "Host requires at least one pattern",
                true,
            )],
        );
    }

    let mut patterns = Vec::with_capacity(values.len());
    let mut discovered = Vec::new();
    let mut diagnostics = Vec::new();
    let mut ambiguous = false;
    for value in values {
        let (negated, pattern) = value
            .strip_prefix('!')
            .map_or((false, value.as_str()), |pattern| (true, pattern));
        if pattern.is_empty() || contains_dangerous_expansion(pattern) {
            ambiguous = true;
            diagnostics.push(diagnostic(
                OpenSshDiagnosticCode::InvalidHostAlias,
                Some(line),
                Some("Host"),
                "Host pattern is empty or contains unsafe expansion syntax",
                true,
            ));
            continue;
        }
        if pattern.contains('[') || pattern.contains(']') {
            ambiguous = true;
            diagnostics.push(diagnostic(
                OpenSshDiagnosticCode::UnsupportedHostPattern,
                Some(line),
                Some("Host"),
                "Bracket Host patterns are not evaluated by the safe import preview",
                true,
            ));
            continue;
        }
        patterns.push(HostPattern {
            pattern: pattern.to_ascii_lowercase(),
            negated,
        });
        if !negated && !pattern.contains(['*', '?']) {
            if is_valid_alias(pattern) {
                discovered.push(pattern.to_string());
            } else {
                diagnostics.push(diagnostic(
                    OpenSshDiagnosticCode::InvalidHostAlias,
                    Some(line),
                    Some("Host"),
                    "Literal Host alias is not a safe ASCII alias",
                    true,
                ));
            }
        }
    }
    (
        BlockSelector::Host {
            patterns,
            ambiguous,
        },
        discovered,
        diagnostics,
    )
}

fn parse_directive(parsed: ParsedLine, line: usize) -> ParsedDirective {
    let keyword = parsed.keyword.to_ascii_lowercase();
    if parsed
        .values
        .iter()
        .any(|value| contains_dangerous_expansion(value))
    {
        return ParsedDirective::Blocking {
            diagnostic: diagnostic(
                OpenSshDiagnosticCode::DangerousExpansion,
                Some(line),
                Some(&parsed.keyword),
                "Command substitution or environment expansion is not imported",
                true,
            ),
            slot: slot_for_keyword(&keyword),
        };
    }

    match keyword.as_str() {
        "hostname" => single_value(parsed, line, DirectiveSlot::HostName, |value| {
            ParsedDirective::HostName { value, line }
        }),
        "user" => single_value(parsed, line, DirectiveSlot::User, |value| {
            ParsedDirective::User { value, line }
        }),
        "port" => single_value(parsed, line, DirectiveSlot::Port, |value| {
            ParsedDirective::Port { value, line }
        }),
        "identityfile" => single_value(parsed, line, DirectiveSlot::IdentityFile, |value| {
            ParsedDirective::IdentityFile { value, line }
        }),
        "proxyjump" => single_value(parsed, line, DirectiveSlot::Route, |value| {
            ParsedDirective::Route {
                value: RouteSource::ProxyJump(value),
                line,
            }
        }),
        "proxycommand" => ParsedDirective::Route {
            value: RouteSource::ProxyCommand(parsed.values),
            line,
        },
        "include" => ParsedDirective::Blocking {
            diagnostic: diagnostic(
                OpenSshDiagnosticCode::IncludeUnsupported,
                Some(line),
                Some(&parsed.keyword),
                "Include is not resolved by the pure-text import preview",
                true,
            ),
            slot: DirectiveSlot::Always,
        },
        keyword if is_ignored_directive(keyword) => ParsedDirective::Ignored,
        _ => ParsedDirective::Blocking {
            diagnostic: diagnostic(
                OpenSshDiagnosticCode::UnsupportedDirective,
                Some(line),
                Some(&parsed.keyword),
                "Directive is outside the safely importable non-secret subset",
                true,
            ),
            slot: DirectiveSlot::Always,
        },
    }
}

fn single_value(
    parsed: ParsedLine,
    line: usize,
    slot: DirectiveSlot,
    make: impl FnOnce(String) -> ParsedDirective,
) -> ParsedDirective {
    if parsed.values.len() == 1 {
        make(parsed.values.into_iter().next().unwrap_or_default())
    } else {
        ParsedDirective::Blocking {
            diagnostic: diagnostic(
                OpenSshDiagnosticCode::InvalidSyntax,
                Some(line),
                Some(&parsed.keyword),
                "Directive requires exactly one value",
                true,
            ),
            slot,
        }
    }
}

fn slot_for_keyword(keyword: &str) -> DirectiveSlot {
    match keyword {
        "hostname" => DirectiveSlot::HostName,
        "user" => DirectiveSlot::User,
        "port" => DirectiveSlot::Port,
        "identityfile" => DirectiveSlot::IdentityFile,
        "proxyjump" | "proxycommand" => DirectiveSlot::Route,
        _ => DirectiveSlot::Always,
    }
}

fn build_candidate(
    alias: &str,
    blocks: &[ConfigBlock],
    document_diagnostics: &[OpenSshDiagnostic],
) -> OpenSshHostCandidate {
    let mut diagnostics = document_diagnostics.to_vec();
    let mut hostname: Option<(String, usize)> = None;
    let mut user: Option<(String, usize)> = None;
    let mut port: Option<(String, usize)> = None;
    let mut route: Option<(RouteSource, usize)> = None;
    let mut identity_file_hints = Vec::new();
    let mut hostname_seen = false;
    let mut user_seen = false;
    let mut port_seen = false;
    let mut route_seen = false;

    for block in blocks {
        if !block.selector.matches(alias) {
            continue;
        }
        diagnostics.extend(block.diagnostics.iter().cloned());
        for directive in &block.directives {
            match directive {
                ParsedDirective::HostName { value, line } if !hostname_seen => {
                    hostname_seen = true;
                    hostname = Some((value.clone(), *line));
                }
                ParsedDirective::User { value, line } if !user_seen => {
                    user_seen = true;
                    user = Some((value.clone(), *line));
                }
                ParsedDirective::Port { value, line } if !port_seen => {
                    port_seen = true;
                    port = Some((value.clone(), *line));
                }
                ParsedDirective::IdentityFile { value, line } => {
                    if value.eq_ignore_ascii_case("none") {
                        identity_file_hints.clear();
                    } else if identity_file_hints.len() == MAX_IDENTITY_FILE_HINTS {
                        if !diagnostics
                            .iter()
                            .any(|item| item.code == OpenSshDiagnosticCode::TooManyIdentityFiles)
                        {
                            diagnostics.push(diagnostic(
                                OpenSshDiagnosticCode::TooManyIdentityFiles,
                                Some(*line),
                                Some("IdentityFile"),
                                "IdentityFile hint limit exceeded",
                                true,
                            ));
                        }
                    } else if is_valid_identity_hint(value) {
                        identity_file_hints.push(value.clone());
                    } else {
                        diagnostics.push(diagnostic(
                            OpenSshDiagnosticCode::InvalidIdentityFile,
                            Some(*line),
                            Some("IdentityFile"),
                            "IdentityFile hint is empty, oversized, or contains unsafe syntax",
                            true,
                        ));
                    }
                }
                ParsedDirective::Route { value, line } if !route_seen => {
                    route_seen = true;
                    route = Some((value.clone(), *line));
                }
                ParsedDirective::Blocking { diagnostic, slot } => match slot {
                    DirectiveSlot::Always | DirectiveSlot::IdentityFile => {
                        diagnostics.push(diagnostic.clone());
                    }
                    DirectiveSlot::HostName if !hostname_seen => {
                        hostname_seen = true;
                        diagnostics.push(diagnostic.clone());
                    }
                    DirectiveSlot::User if !user_seen => {
                        user_seen = true;
                        diagnostics.push(diagnostic.clone());
                    }
                    DirectiveSlot::Port if !port_seen => {
                        port_seen = true;
                        diagnostics.push(diagnostic.clone());
                    }
                    DirectiveSlot::Route if !route_seen => {
                        route_seen = true;
                        diagnostics.push(diagnostic.clone());
                    }
                    DirectiveSlot::HostName
                    | DirectiveSlot::User
                    | DirectiveSlot::Port
                    | DirectiveSlot::Route => {}
                },
                ParsedDirective::HostName { .. }
                | ParsedDirective::User { .. }
                | ParsedDirective::Port { .. }
                | ParsedDirective::Route { .. }
                | ParsedDirective::Ignored => {}
            }
        }
    }

    let parsed_port = parse_port(port.as_ref(), &mut diagnostics);
    let address = parse_hostname(alias, hostname.as_ref(), &mut diagnostics);
    let endpoint = address.and_then(|address| {
        parsed_port.and_then(|port| match Endpoint::parse(&address, port) {
            Ok(endpoint) => Some(OpenSshEndpointPreview {
                address: endpoint.normalized_address().to_string(),
                port: endpoint.port(),
            }),
            Err(_) => {
                diagnostics.push(diagnostic(
                    OpenSshDiagnosticCode::InvalidHostName,
                    hostname.as_ref().map(|(_, line)| *line),
                    Some("HostName"),
                    "HostName is not a supported IPv4, IPv6, or ASCII DNS address",
                    true,
                ));
                None
            }
        })
    });
    let user = parse_user(user.as_ref(), &mut diagnostics);
    let route = parse_route(route.as_ref(), &mut diagnostics);
    let importable = endpoint.is_some() && diagnostics.iter().all(|item| !item.blocking);

    OpenSshHostCandidate {
        alias: alias.to_string(),
        endpoint,
        user,
        identity_file_hints,
        route,
        diagnostics,
        importable,
    }
}

fn parse_hostname(
    alias: &str,
    value: Option<&(String, usize)>,
    diagnostics: &mut Vec<OpenSshDiagnostic>,
) -> Option<String> {
    let Some((value, line)) = value else {
        return Some(alias.to_string());
    };
    let mut output = String::with_capacity(value.len() + alias.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(alias),
            _ => {
                diagnostics.push(diagnostic(
                    OpenSshDiagnosticCode::InvalidHostName,
                    Some(*line),
                    Some("HostName"),
                    "HostName only permits the %% and %h tokens",
                    true,
                ));
                return None;
            }
        }
    }
    Some(output)
}

fn parse_port(
    value: Option<&(String, usize)>,
    diagnostics: &mut Vec<OpenSshDiagnostic>,
) -> Option<u16> {
    let Some((value, line)) = value else {
        return Some(22);
    };
    match value.parse::<u16>() {
        Ok(port) if port != 0 => Some(port),
        _ => {
            diagnostics.push(diagnostic(
                OpenSshDiagnosticCode::InvalidPort,
                Some(*line),
                Some("Port"),
                "Port must be an integer between 1 and 65535",
                true,
            ));
            None
        }
    }
}

fn parse_user(
    value: Option<&(String, usize)>,
    diagnostics: &mut Vec<OpenSshDiagnostic>,
) -> Option<String> {
    let (value, line) = value?;
    if is_valid_user(value) {
        Some(value.clone())
    } else {
        diagnostics.push(diagnostic(
            OpenSshDiagnosticCode::InvalidUser,
            Some(*line),
            Some("User"),
            "User must be a bounded literal ASCII username",
            true,
        ));
        None
    }
}

fn parse_route(
    route: Option<&(RouteSource, usize)>,
    diagnostics: &mut Vec<OpenSshDiagnostic>,
) -> OpenSshRoutePreview {
    let Some((route, line)) = route else {
        return OpenSshRoutePreview::Direct;
    };
    match route {
        RouteSource::ProxyJump(value) => match parse_proxy_jump(value) {
            Ok(hops) => OpenSshRoutePreview::JumpChain(hops),
            Err(message) => {
                diagnostics.push(diagnostic(
                    OpenSshDiagnosticCode::InvalidProxyJump,
                    Some(*line),
                    Some("ProxyJump"),
                    message,
                    true,
                ));
                OpenSshRoutePreview::Direct
            }
        },
        RouteSource::ProxyCommand(values) => match parse_proxy_command(values) {
            Ok(route) => route,
            Err((code, message)) => {
                diagnostics.push(diagnostic(
                    code,
                    Some(*line),
                    Some("ProxyCommand"),
                    message,
                    true,
                ));
                OpenSshRoutePreview::Direct
            }
        },
    }
}

fn parse_proxy_jump(value: &str) -> Result<Vec<OpenSshJumpHopPreview>, &'static str> {
    if value.eq_ignore_ascii_case("none") {
        return Err("ProxyJump none is not a literal jump hop");
    }
    let parts = value.split(',').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > MAX_JUMP_HOPS || parts.iter().any(|part| part.is_empty()) {
        return Err("ProxyJump must contain between one and five literal hops");
    }
    parts.into_iter().map(parse_jump_hop).collect()
}

fn parse_jump_hop(value: &str) -> Result<OpenSshJumpHopPreview, &'static str> {
    if contains_dangerous_expansion(value) || value.contains(['*', '?', '!', ',']) {
        return Err("ProxyJump hop must be literal and cannot contain patterns or expansions");
    }
    let (user, endpoint) = match value.rsplit_once('@') {
        Some((user, endpoint)) => {
            if !is_valid_user(user) || user.contains('@') {
                return Err("ProxyJump user is not a safe literal username");
            }
            (Some(user.to_string()), endpoint)
        }
        None => (None, value),
    };
    let (address, port) = parse_host_port(endpoint, false)?;
    let endpoint = Endpoint::parse(address, port)
        .map_err(|_| "ProxyJump endpoint is not a supported literal endpoint")?;
    Ok(OpenSshJumpHopPreview {
        endpoint: OpenSshEndpointPreview {
            address: endpoint.normalized_address().to_string(),
            port: endpoint.port(),
        },
        user,
    })
}

fn parse_proxy_command(
    values: &[String],
) -> Result<OpenSshRoutePreview, (OpenSshDiagnosticCode, &'static str)> {
    if values.len() == 1 && values[0].eq_ignore_ascii_case("none") {
        return Ok(OpenSshRoutePreview::Direct);
    }
    if values.iter().any(|value| contains_shell_token(value)) {
        return Err((
            OpenSshDiagnosticCode::DangerousToken,
            "ProxyCommand contains shell or command-substitution syntax",
        ));
    }
    if values.len() != 7
        || values[0] != "/usr/bin/nc"
        || values[1] != "-X"
        || values[3] != "-x"
        || values[5] != "%h"
        || values[6] != "%p"
    {
        return Err((
            OpenSshDiagnosticCode::UnsupportedProxyCommand,
            "Only the strict /usr/bin/nc proxy form is safely importable",
        ));
    }
    let (address, port) = parse_host_port(&values[4], true).map_err(|_| {
        (
            OpenSshDiagnosticCode::UnsupportedProxyCommand,
            "ProxyCommand proxy endpoint must be a literal HOST:PORT",
        )
    })?;
    let endpoint = Endpoint::parse(address, port).map_err(|_| {
        (
            OpenSshDiagnosticCode::UnsupportedProxyCommand,
            "ProxyCommand proxy endpoint is invalid",
        )
    })?;
    let proxy = OpenSshEndpointPreview {
        address: endpoint.normalized_address().to_string(),
        port: endpoint.port(),
    };
    if values[2].eq_ignore_ascii_case("connect") {
        Ok(OpenSshRoutePreview::HttpConnect { proxy })
    } else if values[2] == "5" {
        Ok(OpenSshRoutePreview::Socks5 { proxy })
    } else {
        Err((
            OpenSshDiagnosticCode::UnsupportedProxyCommand,
            "nc proxy mode must be connect or 5",
        ))
    }
}

fn parse_host_port(value: &str, require_port: bool) -> Result<(&str, u16), &'static str> {
    let (address, port) = if let Some(after_open) = value.strip_prefix('[') {
        let close = after_open
            .find(']')
            .ok_or("Bracketed endpoint is missing a closing bracket")?;
        let address = &after_open[..close];
        let remainder = &after_open[close + 1..];
        if remainder.is_empty() && !require_port {
            (address, 22)
        } else {
            let port = remainder
                .strip_prefix(':')
                .ok_or("Bracketed endpoint has invalid trailing syntax")?;
            (address, parse_literal_port(port)?)
        }
    } else {
        match value.rsplit_once(':') {
            Some((address, port)) if !address.contains(':') => (address, parse_literal_port(port)?),
            Some(_) => return Err("IPv6 endpoints must use brackets"),
            None if require_port => return Err("Endpoint requires an explicit port"),
            None => (value, 22),
        }
    };
    if address.is_empty() {
        return Err("Endpoint address is empty");
    }
    Ok((address, port))
}

fn parse_literal_port(value: &str) -> Result<u16, &'static str> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or("Port must be between 1 and 65535")
}

fn parse_line(line: &str) -> Result<Option<ParsedLine>, String> {
    let mut tokens = tokenize(line)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let first = tokens.remove(0);
    if let Some((keyword, value)) = first.split_once('=') {
        if keyword.is_empty() {
            return Err("Directive name is empty".to_string());
        }
        if !value.is_empty() {
            tokens.insert(0, value.to_string());
        }
        return Ok(Some(ParsedLine {
            keyword: keyword.to_string(),
            values: tokens,
        }));
    }
    if tokens.first().is_some_and(|token| token == "=") {
        tokens.remove(0);
    }
    Ok(Some(ParsedLine {
        keyword: first,
        values: tokens,
    }))
}

fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut token_started = false;
    let mut quote = None;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match quote {
            Some(quote_character) if character == quote_character => quote = None,
            Some(_) if character == '\\' => {
                let escaped = characters
                    .next()
                    .ok_or_else(|| "Trailing escape in quoted value".to_string())?;
                token.push(escaped);
                token_started = true;
            }
            Some(_) => {
                token.push(character);
                token_started = true;
            }
            None if character == '\'' || character == '"' => {
                quote = Some(character);
                token_started = true;
            }
            None if character == '#' => break,
            None if character.is_whitespace() => {
                if token_started {
                    tokens.push(std::mem::take(&mut token));
                    token_started = false;
                }
            }
            None if character == '\\' => {
                let Some(escaped) = characters.next() else {
                    return Err("Trailing escape in value".to_string());
                };
                if escaped.is_whitespace() || matches!(escaped, '#' | '\\' | '\'' | '"') {
                    token.push(escaped);
                } else {
                    token.push('\\');
                    token.push(escaped);
                }
                token_started = true;
            }
            None => {
                token.push(character);
                token_started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("Unterminated quoted value".to_string());
    }
    if token_started {
        tokens.push(token);
    }
    Ok(tokens)
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for pattern_byte in pattern {
        let mut current = vec![false; value.len() + 1];
        if *pattern_byte == b'*' {
            current[0] = previous[0];
            for index in 1..=value.len() {
                current[index] = previous[index] || current[index - 1];
            }
        } else {
            for index in 1..=value.len() {
                current[index] = previous[index - 1]
                    && (*pattern_byte == b'?'
                        || pattern_byte.eq_ignore_ascii_case(&value[index - 1]));
            }
        }
        previous = current;
    }
    previous[value.len()]
}

fn is_valid_alias(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_valid_user(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.is_ascii()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+' | b'@')
        })
}

fn is_valid_identity_hint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LINE_BYTES
        && !value.chars().any(char::is_control)
        && !contains_dangerous_expansion(value)
        && !value.contains(['`', ';', '|'])
}

fn contains_dangerous_expansion(value: &str) -> bool {
    if value.contains('`') || value.contains("$(") || value.contains("${") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes
        .windows(2)
        .any(|window| window[0] == b'$' && (window[1] == b'_' || window[1].is_ascii_alphanumeric()))
}

fn contains_shell_token(value: &str) -> bool {
    contains_dangerous_expansion(value)
        || value.contains(['`', ';', '|', '>', '<'])
        || value.contains("&&")
}

fn is_ignored_directive(keyword: &str) -> bool {
    matches!(
        keyword,
        "addkeystoagent"
            | "addressfamily"
            | "bindaddress"
            | "canonicalizefallbacklocal"
            | "compression"
            | "connectionattempts"
            | "connecttimeout"
            | "controlmaster"
            | "controlpath"
            | "controlpersist"
            | "dynamicforward"
            | "escapechar"
            | "exitonforwardfailure"
            | "forkafterauthentication"
            | "forwardagent"
            | "forwardx11"
            | "forwardx11trusted"
            | "gatewayports"
            | "ipqos"
            | "localforward"
            | "loglevel"
            | "requesttty"
            | "remoteforward"
            | "sendenv"
            | "serveralivecountmax"
            | "serveraliveinterval"
            | "sessiontype"
            | "setenv"
            | "streamlocalbindmask"
            | "streamlocalbindunlink"
            | "tcpkeepalive"
            | "tunnel"
            | "tunneldevice"
            | "visualhostkey"
    )
}

fn diagnostic(
    code: OpenSshDiagnosticCode,
    line: Option<usize>,
    directive: Option<&str>,
    message: &str,
    blocking: bool,
) -> OpenSshDiagnostic {
    OpenSshDiagnostic {
        code,
        severity: if blocking {
            OpenSshDiagnosticSeverity::Error
        } else {
            OpenSshDiagnosticSeverity::Warning
        },
        line,
        directive: directive.map(str::to_string),
        message: message.to_string(),
        blocking,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_CANDIDATES, MAX_CONFIG_BYTES, OpenSshDiagnosticCode, OpenSshRoutePreview,
        parse_openssh_config_preview,
    };

    fn candidate(input: &str, alias: &str) -> super::OpenSshHostCandidate {
        parse_openssh_config_preview(input)
            .candidates
            .into_iter()
            .find(|candidate| candidate.alias == alias)
            .unwrap_or_else(|| panic!("missing candidate {alias}"))
    }

    #[test]
    fn supports_case_insensitive_keys_equals_comments_and_safe_quotes() {
        let candidate = candidate(
            r#"
                hOsT="Prod"
                  HOSTname = "prod.example.com" # comment
                  uSeR='deploy'
                  pOrT=2222
            "#,
            "Prod",
        );
        assert!(candidate.importable, "{:?}", candidate.diagnostics);
        assert_eq!(candidate.endpoint.unwrap().address, "prod.example.com");
        assert_eq!(candidate.user.as_deref(), Some("deploy"));
    }

    #[test]
    fn applies_source_order_first_value_wins_and_wildcard_defaults() {
        let candidate = candidate(
            r#"
                Host prod
                  User first
                  Port 2200
                Host *
                  User fallback
                  HostName %h.example.com
                  Port 22
                Host prod
                  HostName ignored.example.com
                  Port 2300
            "#,
            "prod",
        );
        assert!(candidate.importable);
        assert_eq!(candidate.user.as_deref(), Some("first"));
        let endpoint = candidate.endpoint.unwrap();
        assert_eq!(endpoint.address, "prod.example.com");
        assert_eq!(endpoint.port, 2200);
    }

    #[test]
    fn identity_file_is_additive_and_never_read() {
        let candidate = candidate(
            r#"
                Host prod
                  IdentityFile ~/.ssh/id_ed25519
                Host *
                  IdentityFile "/keys/team key"
                Host prod
                  IdentityFile ~/.ssh/id_rsa
            "#,
            "prod",
        );
        assert!(candidate.importable);
        assert_eq!(
            candidate.identity_file_hints,
            ["~/.ssh/id_ed25519", "/keys/team key", "~/.ssh/id_rsa"]
        );
    }

    #[test]
    fn identity_file_none_clears_prior_preview_hints() {
        let candidate = candidate(
            r#"
                Host prod
                  IdentityFile ~/.ssh/id_ed25519
                  IdentityFile none
                  IdentityFile ~/.ssh/id_team
            "#,
            "prod",
        );
        assert!(candidate.importable);
        assert_eq!(candidate.identity_file_hints, ["~/.ssh/id_team"]);
    }

    #[test]
    fn only_literal_host_aliases_generate_candidates() {
        let preview = parse_openssh_config_preview(
            "Host *.example.com\n  User default\nHost literal !excluded\n  HostName %h.example.com",
        );
        let aliases = preview
            .candidates
            .iter()
            .map(|candidate| candidate.alias.as_str())
            .collect::<Vec<_>>();
        assert_eq!(aliases, ["literal"]);
        assert!(preview.candidates[0].importable);
    }

    #[test]
    fn parses_bounded_literal_proxy_jump_chain() {
        let candidate = candidate(
            "Host prod\nProxyJump alice@jump.example:2222,[2001:db8::1]:2200",
            "prod",
        );
        assert!(candidate.importable, "{:?}", candidate.diagnostics);
        let OpenSshRoutePreview::JumpChain(hops) = candidate.route else {
            panic!("expected jump chain")
        };
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].user.as_deref(), Some("alice"));
        assert_eq!(hops[1].endpoint.address, "2001:db8::1");
    }

    #[test]
    fn maps_only_strict_nc_proxy_commands_and_accepts_none() {
        let http = candidate(
            "Host http\nProxyCommand /usr/bin/nc -X connect -x proxy.example:8080 %h %p",
            "http",
        );
        assert!(http.importable);
        assert!(matches!(
            http.route,
            OpenSshRoutePreview::HttpConnect { .. }
        ));

        let socks = candidate(
            "Host socks\nProxyCommand /usr/bin/nc -X 5 -x proxy.example:1080 %h %p",
            "socks",
        );
        assert!(socks.importable);
        assert!(matches!(socks.route, OpenSshRoutePreview::Socks5 { .. }));

        let direct = candidate("Host direct\nProxyCommand none", "direct");
        assert!(direct.importable);
        assert_eq!(direct.route, OpenSshRoutePreview::Direct);
    }

    #[test]
    fn rejects_arbitrary_proxy_commands_and_malicious_expansions() {
        for input in [
            "Host prod\nProxyCommand /bin/sh -c 'nc %h %p; touch /tmp/pwned'",
            "Host prod\nHostName $(touch /tmp/pwned)",
            "Host prod\nIdentityFile ${HOME}/.ssh/id_ed25519",
        ] {
            let candidate = candidate(input, "prod");
            assert!(!candidate.importable, "accepted {input}");
            assert!(candidate.diagnostics.iter().any(|item| {
                matches!(
                    item.code,
                    OpenSshDiagnosticCode::DangerousExpansion
                        | OpenSshDiagnosticCode::DangerousToken
                )
            }));
        }
    }

    #[test]
    fn rejects_invalid_endpoint_user_port_and_hostname_tokens() {
        for input in [
            "Host prod\nHostName bad..example",
            "Host prod\nHostName %n.example.com",
            "Host prod\nUser bad/user",
            "Host prod\nPort 0",
        ] {
            assert!(!candidate(input, "prod").importable, "accepted {input}");
        }
    }

    #[test]
    fn include_match_and_security_unknowns_block_affected_candidates() {
        let input = r#"
            Host safe
              HostName safe.example
            Host risky
              Include conf.d/risky
              CertificateFile ~/.ssh/id-cert.pub
            Match exec "true"
              User root
        "#;
        let safe = candidate(input, "safe");
        let risky = candidate(input, "risky");
        assert!(!safe.importable);
        assert!(
            safe.diagnostics
                .iter()
                .any(|item| item.code == OpenSshDiagnosticCode::DangerousToken)
        );
        assert!(!risky.importable);
        assert!(
            risky
                .diagnostics
                .iter()
                .any(|item| item.code == OpenSshDiagnosticCode::IncludeUnsupported)
        );
        assert!(
            risky
                .diagnostics
                .iter()
                .any(|item| item.code == OpenSshDiagnosticCode::UnsupportedDirective)
        );
    }

    #[test]
    fn bracket_patterns_are_explicitly_unsupported_instead_of_misparsed() {
        let candidate = candidate("Host prod [a-z]*\n  User deploy", "prod");
        assert!(!candidate.importable);
        assert!(
            candidate
                .diagnostics
                .iter()
                .any(|item| item.code == OpenSshDiagnosticCode::UnsupportedHostPattern)
        );
    }

    #[test]
    fn source_order_selects_first_of_proxy_jump_and_proxy_command() {
        let candidate = candidate(
            "Host prod\nProxyJump jump.example\nProxyCommand /bin/sh -c bad",
            "prod",
        );
        assert!(candidate.importable);
        assert!(matches!(candidate.route, OpenSshRoutePreview::JumpChain(_)));
    }

    #[test]
    fn enforces_input_and_candidate_bounds() {
        let oversized = "x".repeat(MAX_CONFIG_BYTES + 1);
        let preview = parse_openssh_config_preview(&oversized);
        assert_eq!(
            preview.diagnostics[0].code,
            OpenSshDiagnosticCode::InputTooLarge
        );

        let aliases = (0..=MAX_CANDIDATES)
            .map(|index| format!("host{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let preview = parse_openssh_config_preview(&format!("Host {aliases}"));
        assert_eq!(preview.candidates.len(), MAX_CANDIDATES);
        assert!(
            preview
                .candidates
                .iter()
                .all(|candidate| !candidate.importable)
        );
        assert!(
            preview
                .diagnostics
                .iter()
                .any(|item| item.code == OpenSshDiagnosticCode::TooManyCandidates)
        );
    }

    #[test]
    fn rejects_proxy_jump_patterns_and_more_than_five_hops() {
        for value in [
            "*.example",
            "a.example,b.example,c.example,d.example,e.example,f.example",
            "2001:db8::1",
        ] {
            let candidate = candidate(&format!("Host prod\nProxyJump {value}"), "prod");
            assert!(!candidate.importable, "accepted {value}");
            assert!(
                candidate
                    .diagnostics
                    .iter()
                    .any(|item| item.code == OpenSshDiagnosticCode::InvalidProxyJump)
            );
        }
    }
}
