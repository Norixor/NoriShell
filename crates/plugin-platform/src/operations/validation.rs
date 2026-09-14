use super::PlanError;

pub(super) const MAX_COMMAND_BYTES: usize = 64 * 1024;
pub(super) const MAX_PATH_BYTES: usize = 4 * 1024;

pub(super) fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\"'\"'");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

pub(super) fn text(value: &str, field: &'static str, maximum: usize) -> Result<(), PlanError> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte == 0 || byte < 0x20 || byte == 0x7f)
    {
        return Err(PlanError::InvalidParameter { field });
    }
    if value.len() > maximum {
        return Err(PlanError::ParameterTooLarge { field, maximum });
    }
    Ok(())
}

pub(super) fn identifier(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), PlanError> {
    text(value, field, maximum)?;
    if value.starts_with('-')
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'@' | b':')
        })
    {
        return Err(PlanError::InvalidParameter { field });
    }
    Ok(())
}

pub(super) fn path(value: &str) -> Result<(), PlanError> {
    text(value, "path", MAX_PATH_BYTES)
}

pub(super) fn host(value: &str, field: &'static str) -> Result<(), PlanError> {
    identifier(value, field, 253)?;
    if value.contains('@') {
        return Err(PlanError::InvalidParameter { field });
    }
    Ok(())
}

pub(super) fn http_url(value: &str) -> Result<(), PlanError> {
    text(value, "url", 2048)?;
    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err(PlanError::InvalidParameter { field: "url" });
    }
    let authority = value
        .split_once("://")
        .map(|(_, tail)| tail)
        .unwrap_or_default();
    let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(PlanError::InvalidParameter { field: "url" });
    }
    Ok(())
}

pub(super) fn sha256(value: &str, field: &'static str) -> Result<String, PlanError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PlanError::InvalidParameter { field });
    }
    Ok(value.to_ascii_lowercase())
}

pub(super) fn custom_shell(value: &str) -> Result<(), PlanError> {
    if value.is_empty() || value.len() > 16 * 1024 || value.bytes().any(|byte| byte == 0) {
        return Err(PlanError::InvalidParameter { field: "command" });
    }
    // This is intentionally a narrow known-secret guard, not a claim that arbitrary secrets can
    // be detected. The complete command still requires per-execution review.
    let lower = value.to_ascii_lowercase();
    const KNOWN_SECRET_MARKERS: &[&str] = &[
        "-----begin private key-----",
        "-----begin openssh private key-----",
        "authorization: bearer ",
        "proxy-authorization: bearer ",
        "aws_secret_access_key=",
        "private_key=",
    ];
    if KNOWN_SECRET_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return Err(PlanError::KnownSecretMaterial);
    }
    Ok(())
}
