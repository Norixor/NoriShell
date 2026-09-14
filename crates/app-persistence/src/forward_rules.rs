//! Persistence for versioned port-forward rule templates.

use norishell_core_api::{
    ForwardRuleId, ForwardRuleSummary, HostId, PortForwardRule, WireSequence,
};
use rusqlite::{OptionalExtension, params};

use crate::{
    AppPersistenceError, AppRepository, Result, invalid_column, next_revision, normalized_label,
    parse_id, read_wire_sequence, u64_to_i64, unix_time_ms,
};

const MAX_FORWARD_RULE_JSON_BYTES: usize = 4 * 1024;

impl AppRepository {
    pub fn list_forward_rules(&self) -> Result<Vec<ForwardRuleSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, label, host_id, rule_json, state_version, created_at_ms, updated_at_ms
             FROM forward_rules ORDER BY label COLLATE NOCASE, id",
        )?;
        statement
            .query_map([], read_forward_rule)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn create_forward_rule(
        &mut self,
        rule_id: &ForwardRuleId,
        label: &str,
        rule: &PortForwardRule,
    ) -> Result<ForwardRuleSummary> {
        let label = normalized_label(label)?;
        let host_id = validate_forward_rule(rule)?;
        self.get_host(host_id)?;
        let rule_json = serialize_forward_rule(rule)?;
        let now = unix_time_ms();
        self.connection.execute(
            "INSERT INTO forward_rules
             (id, label, host_id, rule_json, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
            params![rule_id.as_str(), label, host_id.as_str(), rule_json, now],
        )?;
        Ok(ForwardRuleSummary {
            rule_id: rule_id.clone(),
            label,
            host_id: host_id.clone(),
            rule: rule.clone(),
            state_version: WireSequence::new(1),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        })
    }

    pub fn update_forward_rule(
        &mut self,
        rule_id: &ForwardRuleId,
        expected_state_version: WireSequence,
        label: &str,
        rule: &PortForwardRule,
    ) -> Result<ForwardRuleSummary> {
        let label = normalized_label(label)?;
        let host_id = validate_forward_rule(rule)?;
        self.get_host(host_id)?;
        let rule_json = serialize_forward_rule(rule)?;
        let next = next_revision(expected_state_version)?;
        let now = unix_time_ms();
        let changed = self.connection.execute(
            "UPDATE forward_rules SET label = ?1, host_id = ?2, rule_json = ?3,
                    state_version = ?4, updated_at_ms = ?5
             WHERE id = ?6 AND state_version = ?7",
            params![
                label,
                host_id.as_str(),
                rule_json,
                u64_to_i64(next)?,
                now,
                rule_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed == 0 {
            return if self.forward_rule_exists(rule_id)? {
                Err(AppPersistenceError::Conflict)
            } else {
                Err(AppPersistenceError::NotFound)
            };
        }
        let created_at = self.connection.query_row(
            "SELECT created_at_ms FROM forward_rules WHERE id = ?1",
            [rule_id.as_str()],
            |row| row.get(0),
        )?;
        Ok(ForwardRuleSummary {
            rule_id: rule_id.clone(),
            label,
            host_id: host_id.clone(),
            rule: rule.clone(),
            state_version: WireSequence::new(next),
            created_at_unix_ms: created_at,
            updated_at_unix_ms: now,
        })
    }

    pub fn delete_forward_rule(
        &mut self,
        rule_id: &ForwardRuleId,
        expected_state_version: WireSequence,
    ) -> Result<()> {
        let changed = self.connection.execute(
            "DELETE FROM forward_rules WHERE id = ?1 AND state_version = ?2",
            params![rule_id.as_str(), u64_to_i64(expected_state_version.get())?],
        )?;
        if changed == 1 {
            return Ok(());
        }
        if self.forward_rule_exists(rule_id)? {
            Err(AppPersistenceError::Conflict)
        } else {
            Err(AppPersistenceError::NotFound)
        }
    }

    fn forward_rule_exists(&self, rule_id: &ForwardRuleId) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT 1 FROM forward_rules WHERE id = ?1",
                [rule_id.as_str()],
                |_| Ok(true),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
            .map_err(AppPersistenceError::from)
    }
}

fn validate_forward_rule(rule: &PortForwardRule) -> Result<&HostId> {
    let (host_id, bind_address, listen_port, target) = match rule {
        PortForwardRule::Local {
            host_id,
            local_bind_address,
            local_listen_port,
            remote_target_host,
            remote_target_port,
        } => (
            host_id,
            local_bind_address,
            *local_listen_port,
            Some((remote_target_host.as_str(), *remote_target_port)),
        ),
        PortForwardRule::Remote {
            host_id,
            remote_bind_address,
            remote_listen_port,
            local_target_host,
            local_target_port,
        } => (
            host_id,
            remote_bind_address,
            *remote_listen_port,
            Some((local_target_host.as_str(), *local_target_port)),
        ),
        PortForwardRule::Dynamic {
            host_id,
            local_bind_address,
            local_listen_port,
        } => (host_id, local_bind_address, *local_listen_port, None),
    };
    if bind_address.trim().is_empty() || bind_address.len() > 255 {
        return Err(AppPersistenceError::InvalidInput(
            "forward bind address is invalid",
        ));
    }
    if listen_port == 0 {
        return Err(AppPersistenceError::InvalidInput(
            "forward listen port must be non-zero",
        ));
    }
    if let Some((target_host, target_port)) = target
        && (target_host.trim().is_empty() || target_host.len() > 255 || target_port == 0)
    {
        return Err(AppPersistenceError::InvalidInput(
            "forward target is invalid",
        ));
    }
    Ok(host_id)
}

fn serialize_forward_rule(rule: &PortForwardRule) -> Result<String> {
    let encoded = serde_json::to_string(rule)
        .map_err(|_| AppPersistenceError::InvalidInput("forward rule cannot be serialized"))?;
    if encoded.len() > MAX_FORWARD_RULE_JSON_BYTES {
        return Err(AppPersistenceError::InvalidInput(
            "forward rule is too large",
        ));
    }
    Ok(encoded)
}

fn read_forward_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<ForwardRuleSummary> {
    let rule_id = parse_id(row.get::<_, String>(0)?, ForwardRuleId::parse)?;
    let label = row.get::<_, String>(1)?;
    let host_id = parse_id(row.get::<_, String>(2)?, HostId::parse)?;
    let rule_json = row.get::<_, String>(3)?;
    if rule_json.len() > MAX_FORWARD_RULE_JSON_BYTES {
        return Err(invalid_column(AppPersistenceError::InvalidStoredData));
    }
    let rule = serde_json::from_str::<PortForwardRule>(&rule_json).map_err(invalid_column)?;
    let rule_host_id = validate_forward_rule(&rule).map_err(invalid_column)?;
    if rule_host_id != &host_id {
        return Err(invalid_column(AppPersistenceError::InvalidStoredData));
    }
    Ok(ForwardRuleSummary {
        rule_id,
        label,
        host_id,
        rule,
        state_version: read_wire_sequence(row, 4)?,
        created_at_unix_ms: row.get(5)?,
        updated_at_unix_ms: row.get(6)?,
    })
}
