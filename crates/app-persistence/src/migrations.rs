//! Forward-only SQLite schema migrations.

use rusqlite::Connection;

use crate::{AppPersistenceError, Result, SCHEMA_VERSION};

pub(super) fn migrate(connection: &Connection) -> Result<()> {
    let mut current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(AppPersistenceError::UnsupportedSchema(current));
    }
    if current == 0 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE identities (
               id TEXT PRIMARY KEY, label TEXT NOT NULL, username TEXT,
               state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL
             ) STRICT;
             CREATE TABLE credential_refs (
               id TEXT PRIMARY KEY,
               identity_id TEXT NOT NULL REFERENCES identities(id) ON DELETE RESTRICT,
               kind TEXT NOT NULL CHECK(kind IN ('password', 'private_key')),
               secret_ref_id TEXT NOT NULL UNIQUE,
               priority INTEGER NOT NULL CHECK(priority >= 0), label TEXT NOT NULL,
               public_key_algorithm TEXT, public_key_fingerprint TEXT,
               state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL,
               UNIQUE(identity_id, priority)
             ) STRICT;
             CREATE TABLE hosts (
               id TEXT PRIMARY KEY, label TEXT NOT NULL, address TEXT NOT NULL,
               normalized_address TEXT NOT NULL, port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
               username TEXT, identity_id TEXT REFERENCES identities(id) ON DELETE SET NULL,
               favorite INTEGER NOT NULL CHECK(favorite IN (0, 1)), state_version INTEGER NOT NULL,
               created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
             ) STRICT;
             CREATE INDEX hosts_normalized_endpoint ON hosts(normalized_address, port);
             CREATE TABLE known_hosts (
               id TEXT PRIMARY KEY, normalized_address TEXT NOT NULL,
               port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
               key_algorithm TEXT NOT NULL, public_key_base64 TEXT NOT NULL,
               fingerprint_sha256 TEXT NOT NULL, first_trusted_at_ms INTEGER NOT NULL,
               last_verified_at_ms INTEGER NOT NULL, state_version INTEGER NOT NULL,
               UNIQUE(normalized_address, port, key_algorithm)
             ) STRICT;
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
        current = 1;
    }
    if current == 1 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE credential_refs ADD COLUMN passphrase_secret_ref_id TEXT;
             ALTER TABLE credential_refs ADD COLUMN import_operation_id TEXT;
             ALTER TABLE credential_refs ADD COLUMN import_idempotency_key TEXT;
             ALTER TABLE credential_refs ADD COLUMN import_state TEXT NOT NULL DEFAULT 'ready'
               CHECK(import_state IN ('pending', 'ready'));
             CREATE UNIQUE INDEX credential_refs_passphrase_secret_ref_unique
               ON credential_refs(passphrase_secret_ref_id)
               WHERE passphrase_secret_ref_id IS NOT NULL;
             CREATE UNIQUE INDEX credential_refs_import_operation_unique
               ON credential_refs(import_operation_id)
               WHERE import_operation_id IS NOT NULL;
             CREATE UNIQUE INDEX credential_refs_import_idempotency_unique
               ON credential_refs(import_idempotency_key)
               WHERE import_idempotency_key IS NOT NULL;
             CREATE INDEX credential_refs_ready_identity_priority
               ON credential_refs(identity_id, import_state, priority, id);
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
        current = 2;
    }
    if current == 2 {
        migrate_v2_to_v3(connection)?;
        current = 3;
    }
    if current == 3 {
        migrate_v3_to_v4(connection)?;
        current = 4;
    }
    if current == 4 {
        migrate_v4_to_v5(connection)?;
        current = 5;
    }
    if current == 5 {
        migrate_v5_to_v6(connection)?;
        current = 6;
    }
    if current == 6 {
        migrate_v6_to_v7(connection)?;
        current = 7;
    }
    if current == 7 {
        migrate_v7_to_v8(connection)?;
        current = 8;
    }
    if current == 8 {
        migrate_v8_to_v9(connection)?;
        current = 9;
    }
    if current == 9 {
        migrate_v9_to_v10(connection)?;
        current = 10;
    }
    if current == 10 {
        migrate_v10_to_v11(connection)?;
        current = 11;
    }
    if current == 11 {
        migrate_v11_to_v12(connection)?;
        current = 12;
    }
    if current == 12 {
        migrate_v12_to_v13(connection)?;
        current = 13;
    }
    if current == 13 {
        migrate_v13_to_v14(connection)?;
        current = 14;
    }
    if current == 14 {
        migrate_v14_to_v15(connection)?;
        current = 15;
    }
    if current == 15 {
        migrate_v15_to_v16(connection)?;
        current = 16;
    }
    if current == 16 {
        migrate_v16_to_v17(connection)?;
        current = 17;
    }
    if current == 17 {
        migrate_v17_to_v18(connection)?;
        current = 18;
    }
    if current == 18 {
        migrate_v18_to_v19(connection)?;
        current = 19;
    }
    if current == 19 {
        migrate_v19_to_v20(connection)?;
        current = 20;
    }
    if current == 20 {
        migrate_v20_to_v21(connection)?;
        current = 21;
    }
    if current == 21 {
        migrate_v21_to_v22(connection)?;
        current = 22;
    }
    if current == 22 {
        migrate_v22_to_v23(connection)?;
        current = 23;
    }
    if current == 23 {
        migrate_v23_to_v24(connection)?;
        current = 24;
    }
    if current == 24 {
        migrate_v24_to_v25(connection)?;
        current = 25;
    }
    if current == 25 {
        migrate_v25_to_v26(connection)?;
        current = 26;
    }
    if current == 26 {
        migrate_v26_to_v27(connection)?;
        current = 27;
    }
    if current == 27 {
        migrate_v27_to_v28(connection)?;
        current = 28;
    }
    if current == 28 {
        migrate_v28_to_v29(connection)?;
        current = 29;
    }
    if current == 29 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE plugin_catalog_entries ADD COLUMN release_details_json TEXT
               CHECK(release_details_json IS NULL OR length(CAST(release_details_json AS BLOB)) <= 65536);
             PRAGMA user_version = 30;
             COMMIT;",
        )?;
        current = 30;
    }
    if current == 30 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE desktop_profiles (
               id TEXT PRIMARY KEY,
               profile_json TEXT NOT NULL CHECK(length(CAST(profile_json AS BLOB)) <= 8192),
               revision INTEGER NOT NULL CHECK(revision > 0),
               host_id TEXT REFERENCES hosts(id) ON DELETE RESTRICT,
               gateway_host_id TEXT REFERENCES hosts(id) ON DELETE RESTRICT,
               credential_ref_id TEXT REFERENCES credential_refs(id) ON DELETE RESTRICT
             ) STRICT;
             PRAGMA user_version = 31;
             COMMIT;",
        )?;
        current = 31;
    }
    if current == 31 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v31;
             CREATE TABLE plugin_capability_grants (
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
               major_version INTEGER NOT NULL CHECK(major_version >= 0),
               capability TEXT NOT NULL CHECK(capability IN
                 ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
                  'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
                  'clipboard.write', 'terminal.metadata', 'terminal.observe',
                  'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
                  'host.metadata.read', 'host.mutation.propose', 'host.session.request',
                  'remote.inspect', 'remote.exec.request',
                  'network.domain', 'local.files', 'local.process', 'storage.plugin', 'sftp.read', 'sftp.write',
                  'metrics.read', 'ssh.sync')),
               granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
               state_version INTEGER NOT NULL CHECK(state_version >= 1),
               updated_at_ms INTEGER NOT NULL,
               artifact_sha256 TEXT,
               app_version_major INTEGER,
               app_version_minor INTEGER,
               secure_surface_contract_revision INTEGER,
               PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
               CHECK(
                 (artifact_sha256 IS NULL AND app_version_major IS NULL
                   AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
                 OR
                 (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
                   AND app_version_minor IS NOT NULL
                   AND secure_surface_contract_revision IS NOT NULL
                   AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
                   AND app_version_major >= 0 AND app_version_minor >= 0
                   AND secure_surface_contract_revision >= 1)
               )
             ) STRICT;
             INSERT INTO plugin_capability_grants
               (plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision)
             SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                    granted, state_version, updated_at_ms, artifact_sha256,
                    app_version_major, app_version_minor, secure_surface_contract_revision
             FROM plugin_capability_grants_v31;
             DROP TABLE plugin_capability_grants_v31;
             CREATE TABLE IF NOT EXISTS plugin_operation_approval_policies (
               plugin_id TEXT PRIMARY KEY REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               revision INTEGER NOT NULL CHECK(revision >= 1),
               updated_at_ms INTEGER NOT NULL
             ) STRICT;
             CREATE TABLE IF NOT EXISTS plugin_operation_permissions (
               permission_id TEXT PRIMARY KEY CHECK(length(permission_id) = 36),
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL
                 CHECK(length(signer_fingerprint_sha256) = 64
                   AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
                   AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'),
               package_sha256 TEXT NOT NULL
                 CHECK(length(package_sha256) = 64 AND package_sha256 = lower(package_sha256)
                   AND package_sha256 NOT GLOB '*[^0-9a-f]*'),
               operation TEXT NOT NULL CHECK(operation IN
                 ('remote_execute', 'forward_start', 'forward_stop', 'terminal_input',
                  'network_request', 'file_access', 'local_execute', 'host_mutation', 'host_session')),
               capability TEXT NOT NULL CHECK(capability IN
                 ('remote.exec.request', 'terminal.requestInput',
                  'network.domain', 'local.files', 'local.process', 'host.mutation.propose', 'host.session.request')),
               capability_major_version INTEGER NOT NULL CHECK(capability_major_version >= 0),
               capability_revision INTEGER NOT NULL CHECK(capability_revision >= 1),
               artifact_sha256 TEXT NOT NULL
                 CHECK(length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'),
               app_version_major INTEGER NOT NULL CHECK(app_version_major >= 0),
               app_version_minor INTEGER NOT NULL CHECK(app_version_minor >= 0),
               secure_surface_contract_revision INTEGER NOT NULL
                 CHECK(secure_surface_contract_revision >= 1),
               target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32),
               target_label TEXT NOT NULL CHECK(length(CAST(target_label AS BLOB)) BETWEEN 1 AND 512),
               action_label TEXT NOT NULL CHECK(length(CAST(action_label AS BLOB)) BETWEEN 1 AND 128),
               created_at_ms INTEGER NOT NULL,
               UNIQUE(plugin_id, operation, target_fingerprint),
               CHECK(
                 (operation IN ('remote_execute', 'forward_start', 'forward_stop')
                   AND capability = 'remote.exec.request')
                 OR (operation = 'network_request' AND capability = 'network.domain')
                 OR (operation = 'file_access' AND capability = 'local.files')
                 OR (operation = 'local_execute' AND capability = 'local.process')
                 OR (operation = 'terminal_input' AND capability = 'terminal.requestInput')
                 OR (operation = 'host_mutation' AND capability = 'host.mutation.propose')
                 OR (operation = 'host_session' AND capability = 'host.session.request')
               )
             ) STRICT;
             CREATE INDEX IF NOT EXISTS plugin_operation_permissions_plugin_created
               ON plugin_operation_permissions(plugin_id, created_at_ms, permission_id);
             -- v31 databases normally retain this historical table. Creating
             -- an empty copy only repairs an incomplete historical schema so
             -- the one-time transfer below remains forward-only.
             CREATE TABLE IF NOT EXISTS plugin_storage (
               plugin_id TEXT PRIMARY KEY CHECK(length(plugin_id) BETWEEN 1 AND 160),
               value_json TEXT NOT NULL
                 CHECK(length(CAST(value_json AS BLOB)) BETWEEN 2 AND 65536),
               state_version INTEGER NOT NULL CHECK(state_version >= 1),
               updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
             ) STRICT;
             CREATE TABLE IF NOT EXISTS plugin_private_storage (
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
               namespace TEXT NOT NULL CHECK(namespace IN ('kv','blob','cache','meta')),
               storage_key TEXT NOT NULL CHECK(length(CAST(storage_key AS BLOB)) BETWEEN 1 AND 128),
               chunk_index INTEGER NOT NULL DEFAULT 0 CHECK(chunk_index >= -1),
               value_bytes BLOB NOT NULL CHECK(length(value_bytes) <= 65536),
               value_revision INTEGER NOT NULL CHECK(value_revision >= 1),
               store_revision INTEGER NOT NULL CHECK(store_revision >= 1),
               expires_at_ms INTEGER CHECK(expires_at_ms IS NULL OR expires_at_ms >= 0),
               PRIMARY KEY(plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index)
             ) STRICT;
             CREATE INDEX IF NOT EXISTS plugin_private_storage_owner_revision
               ON plugin_private_storage
                 (plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index, store_revision);
             INSERT INTO plugin_private_storage
               (plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index,
                value_bytes, value_revision, store_revision, expires_at_ms)
             SELECT legacy.plugin_id, installation.signer_fingerprint_sha256,
                    'meta', '__norishell_persistent_state_v1', 0,
                    CAST(legacy.value_json AS BLOB), legacy.state_version, legacy.state_version, NULL
             FROM plugin_storage AS legacy
             JOIN plugin_installations AS installation ON installation.plugin_id = legacy.plugin_id;
             INSERT INTO plugin_private_storage
               (plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index,
                value_bytes, value_revision, store_revision, expires_at_ms)
             SELECT legacy.plugin_id, installation.signer_fingerprint_sha256,
                    'meta', '__plugin_private_storage_state_v1', 0,
                    X'0000000000000000', legacy.state_version, legacy.state_version, NULL
             FROM plugin_storage AS legacy
             JOIN plugin_installations AS installation ON installation.plugin_id = legacy.plugin_id;
             DROP TABLE plugin_storage;
             PRAGMA user_version = 32;
             COMMIT;",
        )?;
        current = 32;
    }
    if current == 32 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v32;
             CREATE TABLE plugin_capability_grants (
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
               major_version INTEGER NOT NULL CHECK(major_version >= 0),
               capability TEXT NOT NULL CHECK(capability IN
                 ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
                  'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
                  'clipboard.write', 'terminal.metadata', 'terminal.observe',
                  'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
                  'host.metadata.read', 'host.mutation.propose', 'host.session.request',
                  'remote.inspect', 'remote.exec.request',
                  'network.domain', 'local.files', 'local.process', 'storage.plugin', 'sftp.read', 'sftp.write',
                  'metrics.read', 'ssh.sync')),
               granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
               state_version INTEGER NOT NULL CHECK(state_version >= 1),
               updated_at_ms INTEGER NOT NULL,
               artifact_sha256 TEXT,
               app_version_major INTEGER,
               app_version_minor INTEGER,
               secure_surface_contract_revision INTEGER,
               PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
               CHECK(
                 (artifact_sha256 IS NULL AND app_version_major IS NULL
                   AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
                 OR
                 (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
                   AND app_version_minor IS NOT NULL
                   AND secure_surface_contract_revision IS NOT NULL
                   AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
                   AND app_version_major >= 0 AND app_version_minor >= 0
                   AND secure_surface_contract_revision >= 1)
               )
             ) STRICT;
             INSERT INTO plugin_capability_grants
               (plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision)
             SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision
             FROM plugin_capability_grants_v32;
             DROP TABLE plugin_capability_grants_v32;
             ALTER TABLE plugin_operation_permissions RENAME TO plugin_operation_permissions_v32;
             CREATE TABLE plugin_operation_permissions (
               permission_id TEXT PRIMARY KEY CHECK(length(permission_id) = 36),
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL
                 CHECK(length(signer_fingerprint_sha256) = 64
                   AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
                   AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'),
               package_sha256 TEXT NOT NULL
                 CHECK(length(package_sha256) = 64 AND package_sha256 = lower(package_sha256)
                   AND package_sha256 NOT GLOB '*[^0-9a-f]*'),
               operation TEXT NOT NULL CHECK(operation IN
                 ('remote_execute', 'forward_start', 'forward_stop', 'terminal_input',
                  'network_request', 'file_access', 'local_execute', 'host_mutation', 'host_session')),
               capability TEXT NOT NULL CHECK(capability IN
                 ('remote.exec.request', 'terminal.requestInput',
                  'network.domain', 'local.files', 'local.process', 'host.mutation.propose', 'host.session.request')),
               capability_major_version INTEGER NOT NULL CHECK(capability_major_version >= 0),
               capability_revision INTEGER NOT NULL CHECK(capability_revision >= 1),
               artifact_sha256 TEXT NOT NULL
                 CHECK(length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'),
               app_version_major INTEGER NOT NULL CHECK(app_version_major >= 0),
               app_version_minor INTEGER NOT NULL CHECK(app_version_minor >= 0),
               secure_surface_contract_revision INTEGER NOT NULL
                 CHECK(secure_surface_contract_revision >= 1),
               target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32),
               target_label TEXT NOT NULL CHECK(length(CAST(target_label AS BLOB)) BETWEEN 1 AND 512),
               action_label TEXT NOT NULL CHECK(length(CAST(action_label AS BLOB)) BETWEEN 1 AND 128),
               created_at_ms INTEGER NOT NULL,
               UNIQUE(plugin_id, operation, target_fingerprint),
               CHECK(
                 (operation IN ('remote_execute', 'forward_start', 'forward_stop')
                   AND capability = 'remote.exec.request')
                 OR (operation = 'network_request' AND capability = 'network.domain')
                 OR (operation = 'file_access' AND capability = 'local.files')
                 OR (operation = 'local_execute' AND capability = 'local.process')
                 OR (operation = 'terminal_input' AND capability = 'terminal.requestInput')
                 OR (operation = 'host_mutation' AND capability = 'host.mutation.propose')
                 OR (operation = 'host_session' AND capability = 'host.session.request')
               )
             ) STRICT;
             INSERT INTO plugin_operation_permissions
               (permission_id, plugin_id, signer_fingerprint_sha256, package_sha256,
                operation, capability, capability_major_version, capability_revision,
                artifact_sha256, app_version_major, app_version_minor,
                secure_surface_contract_revision, target_fingerprint, target_label,
                action_label, created_at_ms)
             SELECT permission_id, plugin_id, signer_fingerprint_sha256, package_sha256,
                operation, capability, capability_major_version, capability_revision,
                artifact_sha256, app_version_major, app_version_minor,
                secure_surface_contract_revision, target_fingerprint, target_label,
                action_label, created_at_ms
             FROM plugin_operation_permissions_v32;
             DROP TABLE plugin_operation_permissions_v32;
             CREATE INDEX plugin_operation_permissions_plugin_created
               ON plugin_operation_permissions(plugin_id, created_at_ms, permission_id);
             PRAGMA user_version = 33;
             COMMIT;",
        )?;
    }
    if current <= 33 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             DROP INDEX plugin_operation_permissions_plugin_created;
             ALTER TABLE plugin_operation_permissions RENAME TO plugin_operation_permissions_v33;
             CREATE TABLE plugin_operation_permissions (
               permission_id TEXT PRIMARY KEY CHECK(length(permission_id) = 36),
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL
                 CHECK(length(signer_fingerprint_sha256) = 64
                   AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
                   AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'),
               package_sha256 TEXT NOT NULL
                 CHECK(length(package_sha256) = 64 AND package_sha256 = lower(package_sha256)
                   AND package_sha256 NOT GLOB '*[^0-9a-f]*'),
               operation TEXT NOT NULL CHECK(operation IN
                 ('remote_execute', 'forward_start', 'forward_stop', 'terminal_input',
                  'network_request', 'file_access', 'sftp_read', 'sftp_write', 'local_execute', 'host_mutation', 'host_session')),
               capability TEXT NOT NULL CHECK(capability IN
                 ('remote.exec.request', 'terminal.requestInput',
                  'network.domain', 'local.files', 'sftp.read', 'sftp.write', 'local.process', 'host.mutation.propose', 'host.session.request')),
               capability_major_version INTEGER NOT NULL CHECK(capability_major_version >= 0),
               capability_revision INTEGER NOT NULL CHECK(capability_revision >= 1),
               artifact_sha256 TEXT NOT NULL
                 CHECK(length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'),
               app_version_major INTEGER NOT NULL CHECK(app_version_major >= 0),
               app_version_minor INTEGER NOT NULL CHECK(app_version_minor >= 0),
               secure_surface_contract_revision INTEGER NOT NULL
                 CHECK(secure_surface_contract_revision >= 1),
               target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32),
               target_label TEXT NOT NULL CHECK(length(CAST(target_label AS BLOB)) BETWEEN 1 AND 512),
               action_label TEXT NOT NULL CHECK(length(CAST(action_label AS BLOB)) BETWEEN 1 AND 128),
               created_at_ms INTEGER NOT NULL,
               UNIQUE(plugin_id, operation, target_fingerprint),
               CHECK(
                 (operation IN ('remote_execute', 'forward_start', 'forward_stop')
                   AND capability = 'remote.exec.request')
                 OR (operation = 'network_request' AND capability = 'network.domain')
                 OR (operation = 'file_access' AND capability = 'local.files')
                 OR (operation = 'sftp_read' AND capability = 'sftp.read')
                 OR (operation = 'sftp_write' AND capability = 'sftp.write')
                 OR (operation = 'local_execute' AND capability = 'local.process')
                 OR (operation = 'terminal_input' AND capability = 'terminal.requestInput')
                 OR (operation = 'host_mutation' AND capability = 'host.mutation.propose')
                 OR (operation = 'host_session' AND capability = 'host.session.request')
               )
             ) STRICT;
             INSERT INTO plugin_operation_permissions
               (permission_id, plugin_id, signer_fingerprint_sha256, package_sha256,
                operation, capability, capability_major_version, capability_revision,
                artifact_sha256, app_version_major, app_version_minor,
                secure_surface_contract_revision, target_fingerprint, target_label,
                action_label, created_at_ms)
             SELECT permission_id, plugin_id, signer_fingerprint_sha256, package_sha256,
                operation, capability, capability_major_version, capability_revision,
                artifact_sha256, app_version_major, app_version_minor,
                secure_surface_contract_revision, target_fingerprint, target_label,
                action_label, created_at_ms
             FROM plugin_operation_permissions_v33;
             DROP TABLE plugin_operation_permissions_v33;
             CREATE INDEX plugin_operation_permissions_plugin_created
               ON plugin_operation_permissions(plugin_id, created_at_ms, permission_id);
             PRAGMA user_version = 34;
             COMMIT;",
        )?;
        current = 34;
    }
    if current == 34 {
        migrate_v34_to_v35(connection)?;
        current = 35;
    }
    if current == 35 {
        connection.execute_batch("BEGIN IMMEDIATE;
             ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v35;
             CREATE TABLE plugin_capability_grants (
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
               major_version INTEGER NOT NULL CHECK(major_version >= 0),
               capability TEXT NOT NULL CHECK(capability IN
                 ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
                  'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
                  'clipboard.write', 'terminal.metadata', 'terminal.observe',
                  'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput', 'terminal.provider',
                  'host.metadata.read', 'host.mutation.propose', 'host.session.request',
                  'remote.inspect', 'remote.exec.request',
                  'network.domain', 'local.files', 'local.process', 'storage.plugin',
                  'sftp.read', 'sftp.write', 'credentials.plugin', 'metrics.read', 'ssh.sync')),
               granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
               state_version INTEGER NOT NULL CHECK(state_version >= 1),
               updated_at_ms INTEGER NOT NULL,
               artifact_sha256 TEXT,
               app_version_major INTEGER,
               app_version_minor INTEGER,
               secure_surface_contract_revision INTEGER,
               PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
               CHECK(
                 (artifact_sha256 IS NULL AND app_version_major IS NULL
                   AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
                 OR
                 (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
                   AND app_version_minor IS NOT NULL
                   AND secure_surface_contract_revision IS NOT NULL
                   AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
                   AND app_version_major >= 0 AND app_version_minor >= 0
                   AND secure_surface_contract_revision >= 1)
               )
             ) STRICT;
             INSERT INTO plugin_capability_grants
               (plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision)
             SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision
             FROM plugin_capability_grants_v35;
             DROP TABLE plugin_capability_grants_v35;
             PRAGMA user_version = 36;
             COMMIT;")?;
        current = 36;
    }
    if current == 36 {
        connection.execute_batch("BEGIN IMMEDIATE;
             ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v36;
             CREATE TABLE plugin_capability_grants (
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
               major_version INTEGER NOT NULL CHECK(major_version >= 0),
               capability TEXT NOT NULL CHECK(capability IN
                 ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
                  'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
                  'clipboard.write', 'terminal.metadata', 'terminal.observe',
                  'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput', 'terminal.provider', 'device.serial',
                  'host.metadata.read', 'host.mutation.propose', 'host.session.request',
                  'remote.inspect', 'remote.exec.request',
                  'network.domain', 'local.files', 'local.process', 'storage.plugin',
                  'sftp.read', 'sftp.write', 'credentials.plugin', 'metrics.read', 'ssh.sync')),
               granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
               state_version INTEGER NOT NULL CHECK(state_version >= 1),
               updated_at_ms INTEGER NOT NULL,
               artifact_sha256 TEXT,
               app_version_major INTEGER,
               app_version_minor INTEGER,
               secure_surface_contract_revision INTEGER,
               PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
               CHECK(
                 (artifact_sha256 IS NULL AND app_version_major IS NULL
                   AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
                 OR
                 (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
                   AND app_version_minor IS NOT NULL
                   AND secure_surface_contract_revision IS NOT NULL
                   AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
                   AND app_version_major >= 0 AND app_version_minor >= 0
                   AND secure_surface_contract_revision >= 1)
               )
             ) STRICT;
             INSERT INTO plugin_capability_grants
               (plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision)
             SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision
             FROM plugin_capability_grants_v36;
             DROP TABLE plugin_capability_grants_v36;
             DROP INDEX plugin_operation_permissions_plugin_created;
             ALTER TABLE plugin_operation_permissions RENAME TO plugin_operation_permissions_v36;
             CREATE TABLE plugin_operation_permissions (
               permission_id TEXT PRIMARY KEY CHECK(length(permission_id) = 36),
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL
                 CHECK(length(signer_fingerprint_sha256) = 64
                   AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
                   AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'),
               package_sha256 TEXT NOT NULL
                 CHECK(length(package_sha256) = 64 AND package_sha256 = lower(package_sha256)
                   AND package_sha256 NOT GLOB '*[^0-9a-f]*'),
               operation TEXT NOT NULL CHECK(operation IN
                 ('remote_execute', 'forward_start', 'forward_stop', 'terminal_input',
                  'network_request', 'file_access', 'sftp_read', 'sftp_write', 'serial_access', 'local_execute', 'host_mutation', 'host_session')),
               capability TEXT NOT NULL CHECK(capability IN
                 ('remote.exec.request', 'terminal.requestInput',
                  'network.domain', 'local.files', 'sftp.read', 'sftp.write', 'device.serial', 'local.process', 'host.mutation.propose', 'host.session.request')),
               capability_major_version INTEGER NOT NULL CHECK(capability_major_version >= 0),
               capability_revision INTEGER NOT NULL CHECK(capability_revision >= 1),
               artifact_sha256 TEXT NOT NULL
                 CHECK(length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                   AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'),
               app_version_major INTEGER NOT NULL CHECK(app_version_major >= 0),
               app_version_minor INTEGER NOT NULL CHECK(app_version_minor >= 0),
               secure_surface_contract_revision INTEGER NOT NULL
                 CHECK(secure_surface_contract_revision >= 1),
               target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32),
               target_label TEXT NOT NULL CHECK(length(CAST(target_label AS BLOB)) BETWEEN 1 AND 512),
               action_label TEXT NOT NULL CHECK(length(CAST(action_label AS BLOB)) BETWEEN 1 AND 128),
               created_at_ms INTEGER NOT NULL,
               UNIQUE(plugin_id, operation, target_fingerprint),
               CHECK(
                 (operation IN ('remote_execute', 'forward_start', 'forward_stop')
                   AND capability = 'remote.exec.request')
                 OR (operation = 'network_request' AND capability = 'network.domain')
                 OR (operation = 'file_access' AND capability = 'local.files')
                 OR (operation = 'sftp_read' AND capability = 'sftp.read')
                 OR (operation = 'sftp_write' AND capability = 'sftp.write')
                 OR (operation = 'serial_access' AND capability = 'device.serial')
                 OR (operation = 'local_execute' AND capability = 'local.process')
                 OR (operation = 'terminal_input' AND capability = 'terminal.requestInput')
                 OR (operation = 'host_mutation' AND capability = 'host.mutation.propose')
                 OR (operation = 'host_session' AND capability = 'host.session.request')
               )
             ) STRICT;
             INSERT INTO plugin_operation_permissions
               (permission_id, plugin_id, signer_fingerprint_sha256, package_sha256,
                operation, capability, capability_major_version, capability_revision,
                artifact_sha256, app_version_major, app_version_minor,
                secure_surface_contract_revision, target_fingerprint, target_label,
                action_label, created_at_ms)
             SELECT permission_id, plugin_id, signer_fingerprint_sha256, package_sha256,
                operation, capability, capability_major_version, capability_revision,
                artifact_sha256, app_version_major, app_version_minor,
                secure_surface_contract_revision, target_fingerprint, target_label,
                action_label, created_at_ms
             FROM plugin_operation_permissions_v36;
             DROP TABLE plugin_operation_permissions_v36;
             CREATE INDEX plugin_operation_permissions_plugin_created
               ON plugin_operation_permissions(plugin_id, created_at_ms, permission_id);
             PRAGMA user_version = 37;
             COMMIT;")?;
        current = 37;
    }
    if current == 37 {
        crate::plugin_tasks::migrate_v37_to_v38(connection)?;
        current = 38;
    }
    if current == 38 {
        crate::plugin_tasks::migrate_v38_to_v39(connection)?;
        current = 39;
    }
    if current == 39 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS desktop_profile_password_receipts (
               consumer TEXT NOT NULL CHECK(consumer = 'desktop'),
               stage_id TEXT PRIMARY KEY
                 REFERENCES host_create_password_stages(stage_id) ON DELETE RESTRICT,
               profile_id TEXT NOT NULL UNIQUE,
               identity_id TEXT NOT NULL UNIQUE,
               credential_ref_id TEXT NOT NULL UNIQUE,
               operation_id TEXT NOT NULL UNIQUE,
               idempotency_key TEXT NOT NULL UNIQUE
                 CHECK(length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 128),
               request_json TEXT NOT NULL
                 CHECK(length(CAST(request_json AS BLOB)) BETWEEN 2 AND 16384),
               saved_profile_json TEXT NOT NULL
                 CHECK(length(CAST(saved_profile_json AS BLOB)) BETWEEN 2 AND 8192),
               created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0)
             ) STRICT;
             PRAGMA user_version = 40;
             COMMIT;",
        )?;
        current = 40;
    }
    if current == 40 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE desktop_preferences (
               singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
               revision INTEGER NOT NULL CHECK(revision >= 1),
               window_close_behavior TEXT NOT NULL
                 CHECK(window_close_behavior IN ('hide', 'quit')
                   AND length(CAST(window_close_behavior AS BLOB)) = 4),
               tray_show_status INTEGER NOT NULL CHECK(tray_show_status IN (0, 1)),
               tray_recent_limit INTEGER NOT NULL CHECK(tray_recent_limit BETWEEN 0 AND 10),
               tray_show_host_names INTEGER NOT NULL CHECK(tray_show_host_names IN (0, 1)),
               notification_background_only INTEGER NOT NULL
                 CHECK(notification_background_only IN (0, 1)),
               notification_failure_only INTEGER NOT NULL
                 CHECK(notification_failure_only IN (0, 1)),
               notify_transfer_completed INTEGER NOT NULL
                 CHECK(notify_transfer_completed IN (0, 1)),
               notify_transfer_failed INTEGER NOT NULL CHECK(notify_transfer_failed IN (0, 1)),
               notify_disconnected INTEGER NOT NULL CHECK(notify_disconnected IN (0, 1)),
               updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
             ) STRICT;
             INSERT INTO desktop_preferences
               (singleton, revision, window_close_behavior, tray_show_status, tray_recent_limit,
                tray_show_host_names, notification_background_only, notification_failure_only,
                notify_transfer_completed, notify_transfer_failed, notify_disconnected,
                updated_at_ms)
             VALUES (1, 1, 'hide', 1, 5, 1, 1, 0, 0, 0, 0, 0);
             PRAGMA user_version = 41;
             COMMIT;",
        )?;
        current = 41;
    }
    if current == 41 {
        migrate_v41_to_v42(connection)?;
        current = 42;
    }
    if current == 42 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE offline_import_sagas (
               attempt_id TEXT PRIMARY KEY,
               archive_sha256 TEXT NOT NULL CHECK(length(archive_sha256) = 64),
               plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256) = 64),
               secret_ref_ids_json TEXT NOT NULL CHECK(length(CAST(secret_ref_ids_json AS BLOB)) BETWEEN 2 AND 262144)
             ) STRICT;
             PRAGMA user_version = 43;
             COMMIT;",
        )?;
        current = 43;
    }
    if current == 43 {
        connection.execute_batch(
            "PRAGMA defer_foreign_keys = ON;
             BEGIN IMMEDIATE;
             ALTER TABLE host_monitoring_policies RENAME TO host_monitoring_policies_v43;
             CREATE TABLE host_monitoring_policies (
               host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
               revision INTEGER NOT NULL CHECK(revision >= 1),
               enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
               sample_interval_millis INTEGER NOT NULL
                 CHECK(sample_interval_millis BETWEEN 1500 AND 300000),
               sample_timeout_millis INTEGER NOT NULL
                 CHECK(sample_timeout_millis BETWEEN 500 AND 30000),
               created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
             ) STRICT;
             INSERT INTO host_monitoring_policies
               (host_id, revision, enabled, sample_interval_millis,
                sample_timeout_millis, created_at_ms, updated_at_ms)
             SELECT host_id, revision, enabled,
               CASE WHEN sample_interval_seconds = 15 THEN 1500
                 ELSE sample_interval_seconds * 1000 END,
               sample_timeout_seconds * 1000, created_at_ms, updated_at_ms
             FROM host_monitoring_policies_v43;
             DROP TABLE host_monitoring_policies_v43;
             PRAGMA user_version = 44;
             COMMIT;",
        )?;
        current = 44;
    }
    if current == 44 {
        migrate_v44_to_v45(connection)?;
    }
    Ok(())
}

fn migrate_v34_to_v35(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v34;
         CREATE TABLE plugin_capability_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           capability TEXT NOT NULL CHECK(capability IN
             ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
              'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
              'clipboard.write', 'terminal.metadata', 'terminal.observe',
              'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
              'host.metadata.read', 'host.mutation.propose', 'host.session.request',
              'remote.inspect', 'remote.exec.request',
              'network.domain', 'local.files', 'local.process', 'storage.plugin',
              'sftp.read', 'sftp.write', 'credentials.plugin', 'metrics.read', 'ssh.sync')),
           granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           artifact_sha256 TEXT,
           app_version_major INTEGER,
           app_version_minor INTEGER,
           secure_surface_contract_revision INTEGER,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
           CHECK(
             (artifact_sha256 IS NULL AND app_version_major IS NULL
               AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
             OR
             (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
               AND app_version_minor IS NOT NULL
               AND secure_surface_contract_revision IS NOT NULL
               AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
               AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
               AND app_version_major >= 0 AND app_version_minor >= 0
               AND secure_surface_contract_revision >= 1)
           )
         ) STRICT;
         INSERT INTO plugin_capability_grants
           (plugin_id, signer_fingerprint_sha256, major_version, capability,
            granted, state_version, updated_at_ms, artifact_sha256,
            app_version_major, app_version_minor, secure_surface_contract_revision)
         SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
            granted, state_version, updated_at_ms, artifact_sha256,
            app_version_major, app_version_minor, secure_surface_contract_revision
         FROM plugin_capability_grants_v34;
         DROP TABLE plugin_capability_grants_v34;
         CREATE TABLE plugin_credentials (
           handle TEXT PRIMARY KEY CHECK(length(handle) = 36),
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL
             CHECK(length(signer_fingerprint_sha256) = 64
               AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
               AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'),
           secret_ref_id TEXT NOT NULL UNIQUE CHECK(length(secret_ref_id) = 36),
           operation_id TEXT NOT NULL CHECK(length(CAST(operation_id AS BLOB)) BETWEEN 1 AND 128),
           idempotency_key TEXT NOT NULL CHECK(length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 128),
           label TEXT NOT NULL CHECK(length(CAST(label AS BLOB)) BETWEEN 1 AND 128),
           origin TEXT NOT NULL CHECK(length(CAST(origin AS BLOB)) BETWEEN 1 AND 2048),
           injection_kind TEXT NOT NULL CHECK(injection_kind IN ('bearer', 'header')),
           header_name TEXT CHECK(length(CAST(header_name AS BLOB)) BETWEEN 1 AND 128),
           revision INTEGER NOT NULL CHECK(revision >= 1),
           state TEXT NOT NULL CHECK(state IN ('pending_vault', 'ready', 'cleanup_pending', 'revoked')),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
           UNIQUE(plugin_id, signer_fingerprint_sha256, operation_id),
           UNIQUE(plugin_id, signer_fingerprint_sha256, idempotency_key),
           CHECK((injection_kind = 'bearer' AND header_name IS NULL)
             OR (injection_kind = 'header' AND header_name IS NOT NULL))
         ) STRICT;
         CREATE INDEX plugin_credentials_owner_state
           ON plugin_credentials(plugin_id, signer_fingerprint_sha256, state, created_at_ms);
         PRAGMA user_version = 35;
         COMMIT;",
    )?;
    Ok(())
}

/// A local edit clock survives row deletion and can be replaced with the
/// authenticated source clock in the same transaction as a remote restore.
fn migrate_v44_to_v45(connection: &Connection) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE ssh_sync_local_item_times (
           object_kind TEXT NOT NULL CHECK(object_kind IN (
             'host', 'identity', 'credential', 'secret', 'desktop_profile', 'route',
             'authentication_plan', 'algorithm_policy', 'heartbeat_policy',
             'monitoring_policy', 'login_automation'
           )),
           local_object_id TEXT NOT NULL CHECK(length(local_object_id) = 36),
           update_time_unix_ms INTEGER NOT NULL CHECK(update_time_unix_ms >= 0),
           deleted INTEGER NOT NULL CHECK(deleted IN (0, 1)),
           PRIMARY KEY(object_kind, local_object_id)
         ) STRICT;",
    )?;
    for (kind, table, id_column) in [
        ("host", "hosts", "id"),
        ("identity", "identities", "id"),
        ("credential", "credential_refs", "id"),
        ("route", "host_route_plans", "host_id"),
        (
            "authentication_plan",
            "host_authentication_plans",
            "host_id",
        ),
        ("algorithm_policy", "host_algorithm_policies", "host_id"),
        ("heartbeat_policy", "host_heartbeat_policies", "host_id"),
        ("monitoring_policy", "host_monitoring_policies", "host_id"),
        ("login_automation", "host_login_automations", "host_id"),
    ] {
        transaction.execute_batch(&format!(
            "INSERT INTO ssh_sync_local_item_times
               (object_kind, local_object_id, update_time_unix_ms, deleted)
             SELECT '{kind}', {id_column}, updated_at_ms, 0 FROM {table}
             WHERE updated_at_ms > 0;"
        ))?;
        install_local_item_time_triggers(&transaction, kind, table, id_column, true)?;
    }
    // Pre-existing desktop profiles have no recorded edit time. Leave them
    // unknown until the next real edit; migration time is not an edit time.
    install_local_item_time_triggers(
        &transaction,
        "desktop_profile",
        "desktop_profiles",
        "id",
        false,
    )?;
    transaction.pragma_update(None, "user_version", 45)?;
    transaction.commit()?;
    Ok(())
}

fn install_local_item_time_triggers(
    transaction: &rusqlite::Transaction<'_>,
    kind: &str,
    table: &str,
    id_column: &str,
    has_timestamp: bool,
) -> Result<()> {
    let write_time = if has_timestamp {
        "NEW.updated_at_ms"
    } else {
        "CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)"
    };
    for event in ["INSERT", "UPDATE"] {
        let predicate = if has_timestamp {
            "WHEN NEW.updated_at_ms > 0"
        } else {
            ""
        };
        transaction.execute_batch(&format!(
            "CREATE TRIGGER ssh_sync_clock_{table}_{event} AFTER {event} ON {table} {predicate}
             BEGIN
               INSERT INTO ssh_sync_local_item_times
                 (object_kind, local_object_id, update_time_unix_ms, deleted)
               VALUES ('{kind}', NEW.{id_column}, {write_time}, 0)
               ON CONFLICT(object_kind, local_object_id) DO UPDATE SET
                 update_time_unix_ms = excluded.update_time_unix_ms,
                 deleted = 0;
             END;"
        ))?;
    }
    transaction.execute_batch(&format!(
        "CREATE TRIGGER ssh_sync_clock_{table}_DELETE AFTER DELETE ON {table}
         BEGIN
           INSERT INTO ssh_sync_local_item_times
             (object_kind, local_object_id, update_time_unix_ms, deleted)
           VALUES ('{kind}', OLD.{id_column},
             CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), 1)
           ON CONFLICT(object_kind, local_object_id) DO UPDATE SET
             update_time_unix_ms = excluded.update_time_unix_ms,
             deleted = 1;
         END;"
    ))?;
    Ok(())
}

/// Persists portable desktop selections and admits desktop profile object
/// mappings/memberships without weakening the existing owner-bound keys.
fn migrate_v41_to_v42(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "PRAGMA defer_foreign_keys = ON;
         BEGIN IMMEDIATE;
         ALTER TABLE ssh_sync_profile_states
           ADD COLUMN custom_desktop_profile_ids_json TEXT NOT NULL DEFAULT '[]'
             CHECK(length(CAST(custom_desktop_profile_ids_json AS BLOB)) BETWEEN 2 AND 131072);

         ALTER TABLE ssh_sync_object_mappings RENAME TO ssh_sync_object_mappings_v41;
         CREATE TABLE ssh_sync_object_mappings (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           object_kind TEXT NOT NULL CHECK(
             object_kind IN ('host', 'identity', 'credential', 'secret', 'desktop_profile')
           ),
           portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
           local_object_id TEXT NOT NULL CHECK(length(local_object_id) = 36),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_profile_states(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(
             plugin_id, signer_fingerprint_sha256, profile_id,
             object_kind, portable_object_id
           ),
           UNIQUE(
             plugin_id, signer_fingerprint_sha256, profile_id,
             object_kind, local_object_id
           )
         ) STRICT;
         INSERT INTO ssh_sync_object_mappings
           (plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
            portable_object_id, local_object_id, created_at_ms, updated_at_ms)
         SELECT plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                portable_object_id, local_object_id, created_at_ms, updated_at_ms
         FROM ssh_sync_object_mappings_v41;
         DROP TABLE ssh_sync_object_mappings_v41;
         CREATE INDEX ssh_sync_object_mappings_plugin
           ON ssh_sync_object_mappings(plugin_id, signer_fingerprint_sha256, profile_id);

         ALTER TABLE ssh_sync_scope_memberships RENAME TO ssh_sync_scope_memberships_v41;
         CREATE TABLE ssh_sync_scope_memberships (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           object_kind TEXT NOT NULL CHECK(
             object_kind IN ('host', 'identity', 'credential', 'secret', 'desktop_profile')
           ),
           portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
           membership TEXT NOT NULL CHECK(membership IN ('included', 'excluded')),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_profile_states(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(
             plugin_id, signer_fingerprint_sha256, profile_id,
             object_kind, portable_object_id
           )
         ) STRICT;
         INSERT INTO ssh_sync_scope_memberships
           (plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
            portable_object_id, membership, updated_at_ms)
         SELECT plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                portable_object_id, membership, updated_at_ms
         FROM ssh_sync_scope_memberships_v41;
         DROP TABLE ssh_sync_scope_memberships_v41;

         PRAGMA user_version = 42;
         COMMIT;",
    )?;
    Ok(())
}

/// Binds plugin permission decisions to the exact active artifact and to the
/// app/Secure Surface contract which collected the decision. Only the grant
/// set belonging to the currently active installation is grandfathered into
/// the fixed NoriShell 0.1 / permission-surface-v1 baseline. Other historical
/// rows stay unbound and therefore cannot authorize or carry permissions.
fn migrate_v26_to_v27(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "PRAGMA defer_foreign_keys = ON;
         BEGIN IMMEDIATE;
         ALTER TABLE plugin_host_scope_grants RENAME TO plugin_host_scope_grants_v26;
         ALTER TABLE plugin_host_scope_sets RENAME TO plugin_host_scope_sets_v26;
         ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v26;

         CREATE TABLE plugin_capability_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           capability TEXT NOT NULL CHECK(capability IN
             ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
              'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
              'clipboard.write', 'terminal.metadata', 'terminal.observe',
              'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
              'host.metadata.read', 'host.mutation.propose', 'host.session.request',
              'network.domain', 'storage.plugin', 'sftp.read', 'sftp.write',
              'metrics.read', 'ssh.sync')),
           granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           artifact_sha256 TEXT,
           app_version_major INTEGER,
           app_version_minor INTEGER,
           secure_surface_contract_revision INTEGER,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
           CHECK(
             (artifact_sha256 IS NULL AND app_version_major IS NULL
               AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
             OR
             (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
               AND app_version_minor IS NOT NULL
               AND secure_surface_contract_revision IS NOT NULL
               AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
               AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
               AND app_version_major >= 0 AND app_version_minor >= 0
               AND secure_surface_contract_revision >= 1)
           )
         ) STRICT;
         INSERT INTO plugin_capability_grants
           (plugin_id, signer_fingerprint_sha256, major_version, capability, granted,
            state_version, updated_at_ms, artifact_sha256, app_version_major,
            app_version_minor, secure_surface_contract_revision)
         SELECT plugin_id, signer_fingerprint_sha256, major_version, capability, granted,
                state_version, updated_at_ms, NULL, NULL, NULL, NULL
         FROM plugin_capability_grants_v26;
         DELETE FROM plugin_capability_grants
          WHERE major_version = 1
            AND EXISTS (
              SELECT 1 FROM plugin_installations AS installed
              JOIN plugin_capability_grants_v26 AS grants
                ON grants.plugin_id = installed.plugin_id
               AND grants.signer_fingerprint_sha256 = installed.signer_fingerprint_sha256
             WHERE installed.plugin_id = plugin_capability_grants.plugin_id
               AND installed.signer_fingerprint_sha256 = plugin_capability_grants.signer_fingerprint_sha256
               AND grants.major_version = CAST(
                 substr(installed.active_version, 1, instr(installed.active_version, '.') - 1)
                 AS INTEGER
               )
            );
         INSERT INTO plugin_capability_grants
           (plugin_id, signer_fingerprint_sha256, major_version, capability, granted,
            state_version, updated_at_ms, artifact_sha256, app_version_major,
            app_version_minor, secure_surface_contract_revision)
         SELECT grants.plugin_id, grants.signer_fingerprint_sha256, 1, grants.capability,
                grants.granted, grants.state_version, grants.updated_at_ms,
                installed.package_sha256, 0, 1, 1
         FROM plugin_capability_grants_v26 AS grants
         JOIN plugin_installations AS installed
           ON installed.plugin_id = grants.plugin_id
          AND installed.signer_fingerprint_sha256 = grants.signer_fingerprint_sha256
         WHERE grants.major_version = CAST(
           substr(installed.active_version, 1, instr(installed.active_version, '.') - 1)
           AS INTEGER
         )
         ;

         CREATE TABLE plugin_host_scope_sets (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           artifact_sha256 TEXT,
           app_version_major INTEGER,
           app_version_minor INTEGER,
           secure_surface_contract_revision INTEGER,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version),
           CHECK(
             (artifact_sha256 IS NULL AND app_version_major IS NULL
               AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
             OR
             (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
               AND app_version_minor IS NOT NULL
               AND secure_surface_contract_revision IS NOT NULL
               AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
               AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
               AND app_version_major >= 0 AND app_version_minor >= 0
               AND secure_surface_contract_revision >= 1)
           )
         ) STRICT;
         INSERT INTO plugin_host_scope_sets
           (plugin_id, signer_fingerprint_sha256, major_version, state_version,
            updated_at_ms, artifact_sha256, app_version_major, app_version_minor,
            secure_surface_contract_revision)
         SELECT plugin_id, signer_fingerprint_sha256, major_version, state_version,
                updated_at_ms, NULL, NULL, NULL, NULL
         FROM plugin_host_scope_sets_v26;
         INSERT INTO plugin_host_scope_sets
           (plugin_id, signer_fingerprint_sha256, major_version, state_version,
            updated_at_ms, artifact_sha256, app_version_major, app_version_minor,
            secure_surface_contract_revision)
         SELECT scopes.plugin_id, scopes.signer_fingerprint_sha256, 1,
                scopes.state_version, scopes.updated_at_ms, installed.package_sha256,
                0, 1, 1
         FROM plugin_host_scope_sets_v26 AS scopes
         JOIN plugin_installations AS installed
           ON installed.plugin_id = scopes.plugin_id
          AND installed.signer_fingerprint_sha256 = scopes.signer_fingerprint_sha256
         WHERE scopes.major_version = CAST(
           substr(installed.active_version, 1, instr(installed.active_version, '.') - 1)
           AS INTEGER
         )
         ON CONFLICT(plugin_id, signer_fingerprint_sha256, major_version)
         DO UPDATE SET state_version = excluded.state_version,
                       updated_at_ms = excluded.updated_at_ms,
                       artifact_sha256 = excluded.artifact_sha256,
                       app_version_major = excluded.app_version_major,
                       app_version_minor = excluded.app_version_minor,
                       secure_surface_contract_revision = excluded.secure_surface_contract_revision;

         CREATE TABLE plugin_host_scope_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           capability TEXT NOT NULL CHECK(capability IN
             ('host.metadata.read', 'host.mutation.propose', 'host.session.request')),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, host_id, capability),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, major_version)
             REFERENCES plugin_host_scope_sets(plugin_id, signer_fingerprint_sha256, major_version)
             ON DELETE CASCADE
         ) STRICT;
         INSERT INTO plugin_host_scope_grants
         SELECT * FROM plugin_host_scope_grants_v26;
         DELETE FROM plugin_host_scope_grants
          WHERE major_version = 1
            AND EXISTS (
              SELECT 1 FROM plugin_installations AS installed
              JOIN plugin_host_scope_sets_v26 AS scopes
                ON scopes.plugin_id = installed.plugin_id
               AND scopes.signer_fingerprint_sha256 = installed.signer_fingerprint_sha256
             WHERE installed.plugin_id = plugin_host_scope_grants.plugin_id
               AND installed.signer_fingerprint_sha256 = plugin_host_scope_grants.signer_fingerprint_sha256
               AND scopes.major_version = CAST(
                 substr(installed.active_version, 1, instr(installed.active_version, '.') - 1)
                 AS INTEGER
               )
            );
         INSERT INTO plugin_host_scope_grants
           (plugin_id, signer_fingerprint_sha256, major_version, host_id,
            capability, state_version, updated_at_ms)
         SELECT grants.plugin_id, grants.signer_fingerprint_sha256, 1, grants.host_id,
                grants.capability, grants.state_version, grants.updated_at_ms
         FROM plugin_host_scope_grants_v26 AS grants
         JOIN plugin_installations AS installed
           ON installed.plugin_id = grants.plugin_id
          AND installed.signer_fingerprint_sha256 = grants.signer_fingerprint_sha256
         WHERE grants.major_version = CAST(
           substr(installed.active_version, 1, instr(installed.active_version, '.') - 1)
           AS INTEGER
         );
         DROP TABLE plugin_host_scope_grants_v26;
         DROP TABLE plugin_host_scope_sets_v26;
         DROP TABLE plugin_capability_grants_v26;
         CREATE INDEX plugin_host_scope_grants_host
           ON plugin_host_scope_grants(host_id, plugin_id);
         PRAGMA user_version = 27;
         COMMIT;",
    )?;
    Ok(())
}

pub(super) fn migrate_v2_to_v3(connection: &Connection) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.pragma_update(None, "defer_foreign_keys", "ON")?;
    transaction.execute_batch(
        "ALTER TABLE credential_refs RENAME TO credential_refs_v2;
         CREATE TABLE credential_refs (
           id TEXT PRIMARY KEY,
           identity_id TEXT NOT NULL REFERENCES identities(id) ON DELETE RESTRICT,
           kind TEXT NOT NULL CHECK(kind IN
             ('password', 'private_key', 'keyboard_interactive', 'ssh_agent')),
           priority INTEGER NOT NULL CHECK(priority BETWEEN 0 AND 4294967295),
           label TEXT NOT NULL CHECK(length(label) BETWEEN 1 AND 120),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
           import_operation_id TEXT, import_idempotency_key TEXT,
           import_state TEXT NOT NULL CHECK(import_state IN ('pending', 'ready')),
           UNIQUE(identity_id, priority)
         ) STRICT;
         INSERT INTO credential_refs
           (id, identity_id, kind, priority, label, state_version, created_at_ms,
            updated_at_ms, import_operation_id, import_idempotency_key, import_state)
         SELECT id, identity_id, kind, priority, label, state_version, created_at_ms,
                updated_at_ms, import_operation_id, import_idempotency_key, import_state
         FROM credential_refs_v2;

         CREATE TABLE credential_password_details (
           credential_ref_id TEXT PRIMARY KEY
             REFERENCES credential_refs(id) ON DELETE CASCADE
         ) STRICT;
         CREATE TABLE credential_private_key_details (
           credential_ref_id TEXT PRIMARY KEY
             REFERENCES credential_refs(id) ON DELETE CASCADE,
           public_key_algorithm TEXT,
           public_key_fingerprint TEXT,
           CHECK((public_key_algorithm IS NULL) = (public_key_fingerprint IS NULL))
         ) STRICT;
         CREATE TABLE credential_keyboard_interactive_details (
           credential_ref_id TEXT PRIMARY KEY
             REFERENCES credential_refs(id) ON DELETE CASCADE,
           max_rounds INTEGER NOT NULL DEFAULT 8 CHECK(max_rounds BETWEEN 1 AND 32)
         ) STRICT;
         CREATE TABLE credential_ssh_agent_details (
           credential_ref_id TEXT PRIMARY KEY
             REFERENCES credential_refs(id) ON DELETE CASCADE,
           agent_scope TEXT NOT NULL DEFAULT 'default_environment'
             CHECK(agent_scope = 'default_environment')
         ) STRICT;
         CREATE TABLE credential_secret_slots (
           credential_ref_id TEXT NOT NULL
             REFERENCES credential_refs(id) ON DELETE CASCADE,
           slot_index INTEGER NOT NULL CHECK(slot_index BETWEEN 0 AND 31),
           slot_kind TEXT NOT NULL CHECK(slot_kind IN
             ('password', 'private_key', 'passphrase', 'keyboard_interactive_answer')),
           secret_ref_id TEXT NOT NULL UNIQUE,
           label TEXT NOT NULL CHECK(length(label) BETWEEN 1 AND 120),
           PRIMARY KEY(credential_ref_id, slot_index)
         ) STRICT;
         CREATE UNIQUE INDEX credential_secret_single_slot
           ON credential_secret_slots(credential_ref_id, slot_kind)
           WHERE slot_kind IN ('password', 'private_key', 'passphrase');
         CREATE TRIGGER credential_password_detail_kind
           BEFORE INSERT ON credential_password_details
           WHEN NOT EXISTS(
             SELECT 1 FROM credential_refs
             WHERE id = NEW.credential_ref_id AND kind = 'password'
           )
           BEGIN SELECT RAISE(ABORT, 'credential detail kind mismatch'); END;
         CREATE TRIGGER credential_private_key_detail_kind
           BEFORE INSERT ON credential_private_key_details
           WHEN NOT EXISTS(
             SELECT 1 FROM credential_refs
             WHERE id = NEW.credential_ref_id AND kind = 'private_key'
           )
           BEGIN SELECT RAISE(ABORT, 'credential detail kind mismatch'); END;
         CREATE TRIGGER credential_keyboard_interactive_detail_kind
           BEFORE INSERT ON credential_keyboard_interactive_details
           WHEN NOT EXISTS(
             SELECT 1 FROM credential_refs
             WHERE id = NEW.credential_ref_id AND kind = 'keyboard_interactive'
           )
           BEGIN SELECT RAISE(ABORT, 'credential detail kind mismatch'); END;
         CREATE TRIGGER credential_ssh_agent_detail_kind
           BEFORE INSERT ON credential_ssh_agent_details
           WHEN NOT EXISTS(
             SELECT 1 FROM credential_refs
             WHERE id = NEW.credential_ref_id AND kind = 'ssh_agent'
           )
           BEGIN SELECT RAISE(ABORT, 'credential detail kind mismatch'); END;
         CREATE TRIGGER credential_secret_slot_kind
           BEFORE INSERT ON credential_secret_slots
           WHEN NOT EXISTS(
             SELECT 1 FROM credential_refs
             WHERE id = NEW.credential_ref_id
               AND ((kind = 'password' AND NEW.slot_kind = 'password')
                 OR (kind = 'private_key' AND NEW.slot_kind IN ('private_key', 'passphrase'))
                 OR (kind = 'keyboard_interactive'
                   AND NEW.slot_kind = 'keyboard_interactive_answer'))
           )
           BEGIN SELECT RAISE(ABORT, 'credential secret slot kind mismatch'); END;
         INSERT INTO credential_password_details (credential_ref_id)
           SELECT id FROM credential_refs_v2 WHERE kind = 'password';
         INSERT INTO credential_private_key_details
           (credential_ref_id, public_key_algorithm, public_key_fingerprint)
           SELECT id, public_key_algorithm, public_key_fingerprint
           FROM credential_refs_v2 WHERE kind = 'private_key';
         INSERT INTO credential_secret_slots
           (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
           SELECT id, 0,
                  CASE kind WHEN 'password' THEN 'password' ELSE 'private_key' END,
                  secret_ref_id,
                  CASE kind WHEN 'password' THEN 'Password' ELSE 'Private key' END
           FROM credential_refs_v2;
         INSERT INTO credential_secret_slots
           (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
           SELECT id, 1, 'passphrase', passphrase_secret_ref_id, 'Passphrase'
           FROM credential_refs_v2 WHERE passphrase_secret_ref_id IS NOT NULL;
         DROP TABLE credential_refs_v2;
         CREATE UNIQUE INDEX credential_refs_import_operation_unique
           ON credential_refs(import_operation_id) WHERE import_operation_id IS NOT NULL;
         CREATE UNIQUE INDEX credential_refs_import_idempotency_unique
           ON credential_refs(import_idempotency_key) WHERE import_idempotency_key IS NOT NULL;
         CREATE INDEX credential_refs_ready_identity_priority
           ON credential_refs(identity_id, import_state, priority, id);

         CREATE TABLE host_route_plans (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           ingress_kind TEXT NOT NULL CHECK(ingress_kind IN
             ('direct_tcp', 'http_connect_proxy', 'socks5_proxy')),
           proxy_address TEXT, proxy_normalized_address TEXT,
           proxy_port INTEGER CHECK(proxy_port BETWEEN 1 AND 65535),
           proxy_dns_mode TEXT CHECK(proxy_dns_mode IN ('local', 'proxy')),
           proxy_auth_credential_ref_id TEXT
             REFERENCES credential_refs(id) ON DELETE RESTRICT,
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
           CHECK(
             (ingress_kind = 'direct_tcp' AND proxy_address IS NULL
               AND proxy_normalized_address IS NULL AND proxy_port IS NULL
               AND proxy_dns_mode IS NULL AND proxy_auth_credential_ref_id IS NULL)
             OR
             (ingress_kind = 'http_connect_proxy' AND proxy_address IS NOT NULL
               AND proxy_normalized_address IS NOT NULL AND proxy_port IS NOT NULL
               AND proxy_dns_mode IS NULL)
             OR
             (ingress_kind = 'socks5_proxy' AND proxy_address IS NOT NULL
               AND proxy_normalized_address IS NOT NULL AND proxy_port IS NOT NULL
               AND proxy_dns_mode IS NOT NULL)
           )
         ) STRICT;
         CREATE TABLE host_route_jump_hops (
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4),
           jump_host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE RESTRICT,
           PRIMARY KEY(host_id, ordinal), UNIQUE(host_id, jump_host_id),
           CHECK(host_id <> jump_host_id)
         ) STRICT;
         CREATE TABLE host_authentication_plans (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           mode TEXT NOT NULL CHECK(mode IN ('identity', 'host_override')),
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE host_authentication_credentials (
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 15),
           credential_ref_id TEXT NOT NULL
             REFERENCES credential_refs(id) ON DELETE RESTRICT,
           PRIMARY KEY(host_id, ordinal), UNIQUE(host_id, credential_ref_id)
         ) STRICT;
         CREATE TABLE host_algorithm_policies (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           policy_id TEXT NOT NULL CHECK(length(policy_id) BETWEEN 1 AND 80),
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE host_algorithm_exceptions (
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           category TEXT NOT NULL CHECK(category IN ('key_exchange', 'host_key', 'cipher', 'mac')),
           exception_id TEXT NOT NULL CHECK(length(exception_id) BETWEEN 1 AND 80),
           reason TEXT CHECK(length(reason) <= 240),
           PRIMARY KEY(host_id, category, exception_id)
         ) STRICT;
         CREATE TABLE host_heartbeat_policies (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           mode TEXT NOT NULL CHECK(mode IN ('disabled', 'transport_keepalive', 'shell_heartbeat')),
           interval_seconds INTEGER, reply_timeout_seconds INTEGER,
           failure_threshold INTEGER, shell_payload_text TEXT,
           shell_line_ending TEXT CHECK(shell_line_ending IN ('none', 'cr', 'lf', 'crlf')),
           shell_user_idle_seconds INTEGER,
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
           CHECK(
             (mode = 'disabled' AND interval_seconds IS NULL
               AND reply_timeout_seconds IS NULL AND failure_threshold IS NULL
               AND shell_payload_text IS NULL AND shell_line_ending IS NULL
               AND shell_user_idle_seconds IS NULL)
             OR
             (mode = 'transport_keepalive' AND interval_seconds BETWEEN 10 AND 3600
               AND reply_timeout_seconds BETWEEN 5 AND 60
               AND reply_timeout_seconds < interval_seconds
               AND failure_threshold BETWEEN 1 AND 10
               AND shell_payload_text IS NULL AND shell_line_ending IS NULL
               AND shell_user_idle_seconds IS NULL)
             OR
             (mode = 'shell_heartbeat' AND interval_seconds BETWEEN 30 AND 3600
               AND reply_timeout_seconds IS NULL AND failure_threshold IS NULL
               AND shell_payload_text IS NOT NULL AND length(CAST(shell_payload_text AS BLOB)) <= 1024
               AND shell_line_ending IS NOT NULL
               AND shell_user_idle_seconds BETWEEN 5 AND 3600
               AND NOT(shell_payload_text = '' AND shell_line_ending = 'none'))
           )
         ) STRICT;
         CREATE TABLE host_monitoring_policies (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
           sample_interval_seconds INTEGER NOT NULL CHECK(sample_interval_seconds BETWEEN 10 AND 3600),
           sample_timeout_seconds INTEGER NOT NULL CHECK(sample_timeout_seconds BETWEEN 1 AND 60),
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
           CHECK(sample_timeout_seconds < sample_interval_seconds)
         ) STRICT;
         CREATE TABLE host_monitoring_selections (
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           kind TEXT NOT NULL CHECK(kind IN ('disk_mount', 'network_interface')),
           stable_id TEXT NOT NULL CHECK(length(stable_id) BETWEEN 1 AND 240),
           PRIMARY KEY(host_id, kind, stable_id)
         ) STRICT;
         CREATE TABLE host_login_automations (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE host_login_automation_steps (
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 31),
           kind TEXT NOT NULL CHECK(kind IN ('expect', 'send_text', 'send_secret')),
           literal_text TEXT, append_enter INTEGER CHECK(append_enter IN (0, 1)),
           timeout_seconds INTEGER NOT NULL CHECK(timeout_seconds BETWEEN 1 AND 60),
           secret_ref_id TEXT, secret_label TEXT,
           PRIMARY KEY(host_id, ordinal),
           CHECK(
             (kind = 'expect' AND literal_text IS NOT NULL
               AND length(CAST(literal_text AS BLOB)) BETWEEN 1 AND 4096
               AND append_enter IS NULL AND secret_ref_id IS NULL AND secret_label IS NULL)
             OR
             (kind = 'send_text' AND literal_text IS NOT NULL
               AND length(CAST(literal_text AS BLOB)) <= 4096
               AND append_enter IS NOT NULL AND secret_ref_id IS NULL AND secret_label IS NULL)
             OR
             (kind = 'send_secret' AND literal_text IS NULL
               AND append_enter IS NOT NULL AND secret_ref_id IS NOT NULL
               AND secret_label IS NOT NULL AND length(secret_label) BETWEEN 1 AND 120)
           )
         ) STRICT;

         INSERT INTO host_route_plans
           (host_id, revision, ingress_kind, created_at_ms, updated_at_ms)
           SELECT id, 1, 'direct_tcp', updated_at_ms, updated_at_ms FROM hosts;
         INSERT INTO host_authentication_plans
           (host_id, revision, mode, created_at_ms, updated_at_ms)
           SELECT id, 1, 'identity', updated_at_ms, updated_at_ms FROM hosts;
         INSERT INTO host_algorithm_policies
           (host_id, revision, policy_id, created_at_ms, updated_at_ms)
           SELECT id, 1, 'secure-default', updated_at_ms, updated_at_ms FROM hosts;
         INSERT INTO host_heartbeat_policies
           (host_id, revision, mode, created_at_ms, updated_at_ms)
           SELECT id, 1, 'disabled', updated_at_ms, updated_at_ms FROM hosts;
         INSERT INTO host_monitoring_policies
           (host_id, revision, enabled, sample_interval_seconds, sample_timeout_seconds,
            created_at_ms, updated_at_ms)
           SELECT id, 1, 0, 15, 5, updated_at_ms, updated_at_ms FROM hosts;
         INSERT INTO host_login_automations
           (host_id, revision, enabled, created_at_ms, updated_at_ms)
           SELECT id, 1, 0, updated_at_ms, updated_at_ms FROM hosts;
         PRAGMA user_version = 3;",
    )?;
    let violations: i64 =
        transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v3_to_v4(connection: &Connection) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE host_groups (
           id TEXT PRIMARY KEY,
           label TEXT NOT NULL COLLATE NOCASE UNIQUE CHECK(length(label) BETWEEN 1 AND 120),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE host_tags (
           id TEXT PRIMARY KEY,
           label TEXT NOT NULL COLLATE NOCASE UNIQUE CHECK(length(label) BETWEEN 1 AND 120),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         ALTER TABLE hosts ADD COLUMN group_id TEXT
           REFERENCES host_groups(id) ON DELETE RESTRICT;
         CREATE INDEX hosts_group_id ON hosts(group_id);
         CREATE TABLE host_tag_assignments (
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           tag_id TEXT NOT NULL REFERENCES host_tags(id) ON DELETE RESTRICT,
           PRIMARY KEY(host_id, tag_id)
         ) STRICT;
         CREATE INDEX host_tag_assignments_tag_id ON host_tag_assignments(tag_id, host_id);
         CREATE TABLE host_recent_connections (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           connected_at_ms INTEGER NOT NULL CHECK(connected_at_ms >= 0),
           recency_sequence INTEGER NOT NULL UNIQUE CHECK(recency_sequence >= 1),
           successful_connection_count INTEGER NOT NULL CHECK(successful_connection_count >= 1)
         ) STRICT;
         CREATE INDEX host_recent_connections_recency
           ON host_recent_connections(recency_sequence DESC, host_id);
         PRAGMA user_version = 4;",
    )?;
    let violations: i64 =
        transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v4_to_v5(connection: &Connection) -> Result<()> {
    let existing_agents: i64 = connection.query_row(
        "SELECT COUNT(*) FROM credential_refs WHERE kind = 'ssh_agent'",
        [],
        |row| row.get(0),
    )?;
    if existing_agents != 0 {
        // v4 had no supported creation path and did not retain the selected key blob. Refusing an
        // unverifiable row is safer than binding it to an arbitrary key from the current Agent.
        return Err(AppPersistenceError::InvalidStoredData);
    }
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "DROP TABLE credential_ssh_agent_details;
         CREATE TABLE credential_ssh_agent_details (
           credential_ref_id TEXT PRIMARY KEY
             REFERENCES credential_refs(id) ON DELETE CASCADE,
           agent_scope TEXT NOT NULL CHECK(agent_scope = 'default_environment'),
           public_key_blob BLOB NOT NULL
             CHECK(length(public_key_blob) BETWEEN 1 AND 65536),
           public_key_algorithm TEXT NOT NULL
             CHECK(length(public_key_algorithm) BETWEEN 1 AND 80),
           public_key_fingerprint TEXT NOT NULL
             CHECK(length(public_key_fingerprint) BETWEEN 8 AND 120)
         ) STRICT;
         CREATE TRIGGER credential_ssh_agent_detail_kind
           BEFORE INSERT ON credential_ssh_agent_details
           WHEN NOT EXISTS(
             SELECT 1 FROM credential_refs
             WHERE id = NEW.credential_ref_id AND kind = 'ssh_agent'
           )
           BEGIN SELECT RAISE(ABORT, 'credential detail kind mismatch'); END;
         PRAGMA user_version = 5;",
    )?;
    let violations: i64 =
        transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v5_to_v6(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE terminal_workspace_layout (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           schema_version INTEGER NOT NULL CHECK(schema_version = 1),
           revision INTEGER NOT NULL CHECK(revision >= 1),
           layout_json TEXT NOT NULL
             CHECK(length(CAST(layout_json AS BLOB)) BETWEEN 1 AND 262144),
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         INSERT INTO terminal_workspace_layout
           (singleton, schema_version, revision, layout_json, updated_at_ms)
         VALUES (1, 1, 1, '{\"schemaVersion\":1,\"activeTabId\":null,\"tabs\":[]}', 0);
         PRAGMA user_version = 6;
         COMMIT;",
    )?;
    Ok(())
}

pub(super) fn migrate_v6_to_v7(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE host_login_automations ADD COLUMN confirmed_revision INTEGER
           CHECK(confirmed_revision IS NULL OR confirmed_revision >= 1);
         PRAGMA user_version = 7;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v7_to_v8(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE credential_ssh_agent_details ADD COLUMN identity_kind TEXT NOT NULL
           DEFAULT 'ordinary'
           CHECK(identity_kind IN ('ordinary', 'certificate', 'hardware_key'));
         ALTER TABLE credential_ssh_agent_details ADD COLUMN hardware_application TEXT
           CHECK(hardware_application IS NULL
             OR length(CAST(hardware_application AS BLOB)) BETWEEN 4 AND 255);
         ALTER TABLE credential_ssh_agent_details ADD COLUMN certificate_blob BLOB
           CHECK(certificate_blob IS NULL OR length(certificate_blob) BETWEEN 1 AND 65536);
         ALTER TABLE credential_ssh_agent_details ADD COLUMN certificate_algorithm TEXT
           CHECK(certificate_algorithm IS NULL OR length(certificate_algorithm) BETWEEN 1 AND 80);
         ALTER TABLE credential_ssh_agent_details ADD COLUMN certificate_fingerprint TEXT
           CHECK(certificate_fingerprint IS NULL OR length(certificate_fingerprint) BETWEEN 8 AND 120);
         ALTER TABLE credential_ssh_agent_details
           ADD COLUMN certificate_ca_public_key_fingerprint TEXT
           CHECK(certificate_ca_public_key_fingerprint IS NULL
             OR length(certificate_ca_public_key_fingerprint) BETWEEN 8 AND 120);
         ALTER TABLE credential_ssh_agent_details ADD COLUMN certificate_serial TEXT
           CHECK(certificate_serial IS NULL OR length(certificate_serial) BETWEEN 1 AND 20);
         ALTER TABLE credential_ssh_agent_details ADD COLUMN certificate_key_id TEXT
           CHECK(certificate_key_id IS NULL
             OR length(CAST(certificate_key_id AS BLOB)) <= 1024);
         ALTER TABLE credential_ssh_agent_details
           ADD COLUMN certificate_valid_principals_json TEXT
           CHECK(certificate_valid_principals_json IS NULL
             OR length(CAST(certificate_valid_principals_json AS BLOB)) <= 65536);
         ALTER TABLE credential_ssh_agent_details ADD COLUMN certificate_type TEXT
           CHECK(certificate_type IS NULL OR certificate_type = 'user');
         ALTER TABLE credential_ssh_agent_details
           ADD COLUMN certificate_valid_after_unix_seconds INTEGER
           CHECK(certificate_valid_after_unix_seconds IS NULL
             OR certificate_valid_after_unix_seconds >= 0);
         ALTER TABLE credential_ssh_agent_details
           ADD COLUMN certificate_valid_before_unix_seconds INTEGER
           CHECK(certificate_valid_before_unix_seconds IS NULL
             OR certificate_valid_before_unix_seconds >= 0);
         ALTER TABLE credential_ssh_agent_details
           ADD COLUMN certificate_critical_options_json TEXT
           CHECK(certificate_critical_options_json IS NULL
             OR length(CAST(certificate_critical_options_json AS BLOB)) <= 262144);
         ALTER TABLE credential_ssh_agent_details
           ADD COLUMN certificate_extensions_json TEXT
           CHECK(certificate_extensions_json IS NULL
             OR length(CAST(certificate_extensions_json AS BLOB)) <= 524288);
         PRAGMA user_version = 8;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v8_to_v9(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE plugin_catalog_trust (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           root_key_id TEXT NOT NULL CHECK(length(root_key_id) BETWEEN 1 AND 120),
           sequence INTEGER NOT NULL CHECK(sequence >= 1),
           payload_sha256 TEXT NOT NULL CHECK(length(payload_sha256) = 64),
           catalog_signature_base64 TEXT NOT NULL
             CHECK(length(catalog_signature_base64) BETWEEN 80 AND 120),
           catalog_revision TEXT NOT NULL CHECK(length(catalog_revision) BETWEEN 1 AND 120),
           expires_at_ms INTEGER NOT NULL,
           verified_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE plugin_catalog_entries (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           version TEXT NOT NULL CHECK(length(version) BETWEEN 1 AND 80),
           name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 160),
           publisher TEXT NOT NULL CHECK(length(publisher) BETWEEN 1 AND 160),
           protocol_major INTEGER NOT NULL CHECK(protocol_major BETWEEN 0 AND 65535),
           protocol_minor INTEGER NOT NULL CHECK(protocol_minor BETWEEN 0 AND 65535),
           platform TEXT NOT NULL CHECK(length(platform) BETWEEN 1 AND 40),
           architectures_json TEXT NOT NULL CHECK(length(architectures_json) <= 2048),
           package_url TEXT NOT NULL CHECK(length(package_url) BETWEEN 9 AND 2048),
           package_size INTEGER NOT NULL CHECK(package_size > 0),
           package_sha256 TEXT NOT NULL CHECK(length(package_sha256) = 64),
           publisher_key_base64 TEXT NOT NULL CHECK(length(publisher_key_base64) BETWEEN 40 AND 64),
           publisher_signature_base64 TEXT NOT NULL
             CHECK(length(publisher_signature_base64) BETWEEN 80 AND 120),
           capabilities_json TEXT NOT NULL CHECK(length(capabilities_json) <= 1024),
           minimum_app_version TEXT NOT NULL CHECK(length(minimum_app_version) BETWEEN 1 AND 80),
           published_at_ms INTEGER NOT NULL,
           catalog_sequence INTEGER NOT NULL CHECK(catalog_sequence >= 1),
           PRIMARY KEY(plugin_id, version)
         ) STRICT;
         CREATE TABLE plugin_installations (
           plugin_id TEXT PRIMARY KEY CHECK(length(plugin_id) BETWEEN 1 AND 160),
           name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 160),
           publisher TEXT NOT NULL CHECK(length(publisher) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           active_version TEXT NOT NULL CHECK(length(active_version) BETWEEN 1 AND 80),
           package_sha256 TEXT NOT NULL CHECK(length(package_sha256) = 64),
           capabilities_json TEXT NOT NULL CHECK(length(capabilities_json) <= 1024),
           state TEXT NOT NULL CHECK(state IN
             ('enabled', 'disabled', 'update_available', 'crashed', 'quarantined', 'incompatible')),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           installed_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE plugin_installed_versions (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           version TEXT NOT NULL CHECK(length(version) BETWEEN 1 AND 80),
           package_size INTEGER NOT NULL CHECK(package_size > 0),
           package_sha256 TEXT NOT NULL CHECK(length(package_sha256) = 64),
           publisher_key_base64 TEXT NOT NULL CHECK(length(publisher_key_base64) BETWEEN 40 AND 64),
           publisher_signature_base64 TEXT NOT NULL
             CHECK(length(publisher_signature_base64) BETWEEN 80 AND 120),
           capabilities_json TEXT NOT NULL CHECK(length(capabilities_json) <= 1024),
           installed_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, version)
         ) STRICT;
         CREATE TABLE plugin_capability_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           capability TEXT NOT NULL CHECK(capability IN
             ('ui.panel', 'clipboard.write', 'terminal.metadata', 'terminal.observe',
              'terminal.proposeInput', 'terminal.requestInput')),
           granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
         ) STRICT;
         CREATE TABLE plugin_operations (
           operation_id TEXT PRIMARY KEY,
           plugin_id TEXT,
           kind TEXT NOT NULL CHECK(kind IN
             ('catalog_refresh', 'install', 'update', 'disable', 'uninstall')),
           state TEXT NOT NULL CHECK(state IN
             ('pending', 'running', 'awaiting_capabilities', 'succeeded', 'failed', 'cancelled')),
           phase TEXT NOT NULL CHECK(phase IN
             ('resolve', 'download', 'verify_catalog', 'verify_package',
              'awaiting_capabilities', 'staged', 'filesystem_activated',
              'database_committed', 'reconcile_required', 'completed')),
           idempotency_key TEXT NOT NULL UNIQUE CHECK(length(idempotency_key) BETWEEN 1 AND 160),
           request_fingerprint_sha256 TEXT NOT NULL CHECK(length(request_fingerprint_sha256) = 64),
           candidate_version TEXT CHECK(candidate_version IS NULL OR length(candidate_version) BETWEEN 1 AND 80),
           expected_active_version TEXT
             CHECK(expected_active_version IS NULL OR length(expected_active_version) BETWEEN 1 AND 80),
           error_code TEXT CHECK(error_code IS NULL OR length(error_code) <= 120),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE INDEX plugin_operations_reconcile
           ON plugin_operations(state, phase, updated_at_ms);
         CREATE TABLE plugin_audit (
           audit_id INTEGER PRIMARY KEY,
           plugin_id TEXT,
           operation_id TEXT,
           action TEXT NOT NULL CHECK(length(action) BETWEEN 1 AND 80),
           outcome TEXT NOT NULL CHECK(length(outcome) BETWEEN 1 AND 40),
           detail_code TEXT CHECK(detail_code IS NULL OR length(detail_code) <= 120),
           created_at_ms INTEGER NOT NULL
         ) STRICT;
         PRAGMA user_version = 9;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v9_to_v10(connection: &Connection) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.pragma_update(None, "defer_foreign_keys", "ON")?;
    let invalid_selections: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM host_monitoring_selections
         WHERE NOT (
           (kind = 'disk_mount' AND stable_id = 'root') OR
           (kind = 'network_interface' AND stable_id = 'aggregateNonLoopback')
         )",
        [],
        |row| row.get(0),
    )?;
    let duplicate_selections: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM (
           SELECT host_id, kind FROM host_monitoring_selections
           GROUP BY host_id, kind HAVING COUNT(*) <> 1
         )",
        [],
        |row| row.get(0),
    )?;
    if invalid_selections != 0 || duplicate_selections != 0 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    transaction.execute_batch(
        "ALTER TABLE host_monitoring_selections RENAME TO host_monitoring_selections_v9;
         ALTER TABLE host_monitoring_policies RENAME TO host_monitoring_policies_v9;

         CREATE TABLE host_monitoring_policies (
           host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
           sample_interval_seconds INTEGER NOT NULL
             CHECK(sample_interval_seconds BETWEEN 5 AND 300),
           sample_timeout_seconds INTEGER NOT NULL
             CHECK(sample_timeout_seconds BETWEEN 2 AND 30),
           created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
           CHECK(sample_timeout_seconds < sample_interval_seconds)
         ) STRICT;
         WITH bounded AS (
           SELECT host_id, revision, enabled,
             CASE
               WHEN sample_interval_seconds < 5 THEN 5
               WHEN sample_interval_seconds > 300 THEN 300
               ELSE sample_interval_seconds
             END AS sample_interval_seconds,
             CASE
               WHEN sample_timeout_seconds < 2 THEN 2
               WHEN sample_timeout_seconds > 30 THEN 30
               ELSE sample_timeout_seconds
             END AS sample_timeout_seconds,
             created_at_ms, updated_at_ms
           FROM host_monitoring_policies_v9
         )
         INSERT INTO host_monitoring_policies
           (host_id, revision, enabled, sample_interval_seconds,
            sample_timeout_seconds, created_at_ms, updated_at_ms)
         SELECT host_id, revision, enabled, sample_interval_seconds,
           CASE
             WHEN sample_timeout_seconds < sample_interval_seconds
               THEN sample_timeout_seconds
             ELSE sample_interval_seconds - 1
           END,
           created_at_ms, updated_at_ms
         FROM bounded;

         CREATE TABLE host_monitoring_selections (
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           kind TEXT NOT NULL CHECK(kind IN ('disk_mount', 'network_interface')),
           stable_id TEXT NOT NULL,
           PRIMARY KEY(host_id, kind),
           CHECK(
             (kind = 'disk_mount' AND stable_id = 'root') OR
             (kind = 'network_interface' AND stable_id = 'aggregateNonLoopback')
           )
         ) STRICT;
         INSERT INTO host_monitoring_selections (host_id, kind, stable_id)
           SELECT host_id, kind, stable_id FROM host_monitoring_selections_v9;
         INSERT INTO host_monitoring_selections (host_id, kind, stable_id)
           SELECT id, 'disk_mount', 'root' FROM hosts
           WHERE NOT EXISTS (
             SELECT 1 FROM host_monitoring_selections
             WHERE host_id = hosts.id AND kind = 'disk_mount'
           );
         INSERT INTO host_monitoring_selections (host_id, kind, stable_id)
           SELECT id, 'network_interface', 'aggregateNonLoopback' FROM hosts
           WHERE NOT EXISTS (
             SELECT 1 FROM host_monitoring_selections
             WHERE host_id = hosts.id AND kind = 'network_interface'
           );
         DROP TABLE host_monitoring_selections_v9;
         DROP TABLE host_monitoring_policies_v9;
         PRAGMA user_version = 10;",
    )?;
    let violations: i64 =
        transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    transaction.commit()?;
    Ok(())
}

fn migrate_v10_to_v11(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE login_automation_secret_stages (
           stage_id TEXT PRIMARY KEY,
           secret_ref_id TEXT NOT NULL UNIQUE,
           host_id TEXT NOT NULL,
           expected_automation_revision INTEGER NOT NULL
             CHECK(expected_automation_revision >= 1),
           operation_id TEXT NOT NULL UNIQUE,
           idempotency_key TEXT NOT NULL UNIQUE
             CHECK(length(idempotency_key) BETWEEN 1 AND 160),
           label TEXT NOT NULL CHECK(length(label) BETWEEN 1 AND 120),
           expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms >= 0),
           state TEXT NOT NULL CHECK(state IN
             ('pending_vault', 'staged', 'cleanup_pending', 'cancelled', 'consumed')),
           created_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE INDEX login_automation_secret_stages_cleanup
           ON login_automation_secret_stages(state, expires_at_ms);
         PRAGMA user_version = 11;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v11_to_v12(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE forward_rules (
           id TEXT PRIMARY KEY,
           label TEXT NOT NULL CHECK(length(label) BETWEEN 1 AND 120),
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE RESTRICT,
           rule_json TEXT NOT NULL CHECK(length(rule_json) BETWEEN 2 AND 4096),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
         ) STRICT;
         CREATE INDEX forward_rules_host_id ON forward_rules(host_id);
         PRAGMA user_version = 12;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v12_to_v13(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE plugin_installed_versions RENAME TO plugin_installed_versions_v12;
         CREATE TABLE plugin_installed_versions (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           version TEXT NOT NULL CHECK(length(version) BETWEEN 1 AND 80),
           package_size INTEGER NOT NULL CHECK(package_size > 0),
           package_sha256 TEXT NOT NULL CHECK(length(package_sha256) = 64),
           publisher_key_base64 TEXT NOT NULL
             CHECK(length(publisher_key_base64) = 0 OR length(publisher_key_base64) BETWEEN 40 AND 64),
           publisher_signature_base64 TEXT NOT NULL
             CHECK(length(publisher_signature_base64) = 0 OR length(publisher_signature_base64) BETWEEN 80 AND 120),
           capabilities_json TEXT NOT NULL CHECK(length(capabilities_json) <= 1024),
           installed_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, version)
         ) STRICT;
         INSERT INTO plugin_installed_versions
           SELECT * FROM plugin_installed_versions_v12;
         DROP TABLE plugin_installed_versions_v12;
         PRAGMA user_version = 13;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v13_to_v14(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE host_configured_create_operations (
           operation_id TEXT PRIMARY KEY,
           idempotency_key TEXT NOT NULL UNIQUE
             CHECK(length(idempotency_key) BETWEEN 1 AND 160),
           request_json TEXT NOT NULL
             CHECK(length(CAST(request_json AS BLOB)) BETWEEN 2 AND 262144),
           host_id TEXT NOT NULL UNIQUE,
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0)
         ) STRICT;
         CREATE TABLE host_create_password_stages (
           stage_id TEXT PRIMARY KEY,
           secret_ref_id TEXT NOT NULL UNIQUE,
           operation_id TEXT NOT NULL UNIQUE,
           idempotency_key TEXT NOT NULL UNIQUE
             CHECK(length(idempotency_key) BETWEEN 1 AND 160),
           identity_label TEXT NOT NULL CHECK(length(identity_label) BETWEEN 1 AND 120),
           credential_label TEXT NOT NULL CHECK(length(credential_label) BETWEEN 1 AND 120),
           expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms >= 0),
           state TEXT NOT NULL CHECK(state IN
             ('pending_vault', 'staged', 'cleanup_pending', 'cancelled', 'consumed')),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
         ) STRICT;
         CREATE INDEX host_create_password_stages_cleanup
           ON host_create_password_stages(state, expires_at_ms);
         PRAGMA user_version = 14;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v14_to_v15(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v14;
         CREATE TABLE plugin_capability_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           capability TEXT NOT NULL CHECK(capability IN
             ('ui.panel', 'clipboard.write', 'terminal.metadata', 'terminal.observe',
              'terminal.proposeInput', 'terminal.requestInput')),
           granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
         ) STRICT;
         INSERT INTO plugin_capability_grants
           (plugin_id, signer_fingerprint_sha256, major_version, capability,
            granted, state_version, updated_at_ms)
           SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                  granted, state_version, updated_at_ms
           FROM plugin_capability_grants_v14;
         DROP TABLE plugin_capability_grants_v14;
         PRAGMA user_version = 15;
         COMMIT;",
    )?;
    Ok(())
}

/// Repair databases that reached v15 before the password staging table was introduced.
/// Add only non-secret lifecycle metadata; never read or rewrite the Vault payload.
fn migrate_v15_to_v16(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS host_create_password_stages (
           stage_id TEXT PRIMARY KEY,
           secret_ref_id TEXT NOT NULL UNIQUE,
           operation_id TEXT NOT NULL UNIQUE,
           idempotency_key TEXT NOT NULL UNIQUE
             CHECK(length(idempotency_key) BETWEEN 1 AND 160),
           identity_label TEXT NOT NULL CHECK(length(identity_label) BETWEEN 1 AND 120),
           credential_label TEXT NOT NULL CHECK(length(credential_label) BETWEEN 1 AND 120),
           expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms >= 0),
           state TEXT NOT NULL CHECK(state IN
             ('pending_vault', 'staged', 'cleanup_pending', 'cancelled', 'consumed')),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
         ) STRICT;
         CREATE INDEX IF NOT EXISTS host_create_password_stages_cleanup
           ON host_create_password_stages(state, expires_at_ms);
         PRAGMA user_version = 16;
         COMMIT;",
    )?;
    Ok(())
}

/// Expands the durable capability allowlist without granting any capability.
/// Existing grants retain their signer/major/version fences verbatim.
fn migrate_v16_to_v17(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v16;
         CREATE TABLE plugin_capability_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           capability TEXT NOT NULL CHECK(capability IN
             ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
              'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
              'clipboard.write', 'terminal.metadata', 'terminal.observe',
              'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
              'host.metadata.read', 'host.mutation.propose', 'host.session.request',
              'network.domain', 'storage.plugin', 'sftp.read', 'sftp.write',
              'metrics.read')),
           granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
         ) STRICT;
         INSERT INTO plugin_capability_grants
           (plugin_id, signer_fingerprint_sha256, major_version, capability,
            granted, state_version, updated_at_ms)
           SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                  granted, state_version, updated_at_ms
           FROM plugin_capability_grants_v16;
         DROP TABLE plugin_capability_grants_v16;
         PRAGMA user_version = 17;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v17_to_v18(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE plugin_host_scope_sets (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version)
         ) STRICT;
         CREATE TABLE plugin_host_scope_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
           capability TEXT NOT NULL CHECK(capability IN
             ('host.metadata.read', 'host.mutation.propose', 'host.session.request')),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, host_id, capability),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, major_version)
             REFERENCES plugin_host_scope_sets(plugin_id, signer_fingerprint_sha256, major_version)
             ON DELETE CASCADE
         ) STRICT;
         CREATE INDEX plugin_host_scope_grants_host
           ON plugin_host_scope_grants(host_id, plugin_id);
         PRAGMA user_version = 18;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v18_to_v19(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS plugin_storage (
           plugin_id TEXT PRIMARY KEY CHECK(length(plugin_id) BETWEEN 1 AND 160),
           value_json TEXT NOT NULL
             CHECK(length(CAST(value_json AS BLOB)) BETWEEN 2 AND 65536),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
         ) STRICT;
         PRAGMA user_version = 19;
         COMMIT;",
    )?;
    Ok(())
}

/// Expands the durable capability allowlist without changing any decision.
/// Existing signer, major-version, grant revision and update-time fences are
/// copied verbatim into the rebuilt STRICT table.
fn migrate_v19_to_v20(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v19;
         CREATE TABLE plugin_capability_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           capability TEXT NOT NULL CHECK(capability IN
             ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
              'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
              'clipboard.write', 'terminal.metadata', 'terminal.observe',
              'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
              'host.metadata.read', 'host.mutation.propose', 'host.session.request',
              'network.domain', 'storage.plugin', 'sftp.read', 'sftp.write',
              'metrics.read', 'ssh.sync')),
           granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
         ) STRICT;
         INSERT INTO plugin_capability_grants
           (plugin_id, signer_fingerprint_sha256, major_version, capability,
            granted, state_version, updated_at_ms)
           SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                  granted, state_version, updated_at_ms
           FROM plugin_capability_grants_v19;
         DROP TABLE plugin_capability_grants_v19;
         PRAGMA user_version = 20;
         COMMIT;",
    )?;
    Ok(())
}

/// Adds the non-secret durable marker for SSH sync restore reconciliation.
/// Metadata publication deletes the marker in the same final transaction, so
/// a remaining row always means that no restore metadata commit completed.
fn migrate_v20_to_v21(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS ssh_sync_restore_sagas (
           attempt_id TEXT PRIMARY KEY,
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           bundle_sha256 TEXT NOT NULL CHECK(
             length(bundle_sha256) = 64 AND bundle_sha256 = lower(bundle_sha256)
           ),
           plan_sha256 TEXT NOT NULL CHECK(
             length(plan_sha256) = 64 AND plan_sha256 = lower(plan_sha256)
           ),
           secret_ref_ids_json TEXT NOT NULL CHECK(
             length(CAST(secret_ref_ids_json AS BLOB)) BETWEEN 2 AND 262144
           ),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
         ) STRICT;
         CREATE INDEX IF NOT EXISTS ssh_sync_restore_sagas_plugin_profile
           ON ssh_sync_restore_sagas(plugin_id, profile_id, created_at_ms);
         PRAGMA user_version = 21;
         COMMIT;",
    )?;
    Ok(())
}

/// Stores only the non-secret, signer-bound local control plane for one SSH
/// sync profile. The actual sync key remains in Vault behind its SecretRef.
fn migrate_v21_to_v22(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS ssh_sync_profile_states (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           scope_mode TEXT NOT NULL CHECK(scope_mode IN ('all_eligible', 'custom')),
           custom_host_ids_json TEXT NOT NULL CHECK(
             length(CAST(custom_host_ids_json AS BLOB)) BETWEEN 2 AND 131072
           ),
           custom_credential_ref_ids_json TEXT NOT NULL CHECK(
             length(CAST(custom_credential_ref_ids_json AS BLOB)) BETWEEN 2 AND 524288
           ),
           sync_key_secret_ref_id TEXT CHECK(
             sync_key_secret_ref_id IS NULL
             OR length(sync_key_secret_ref_id) BETWEEN 1 AND 160
           ),
           password_wrapped_sync_key_envelope BLOB CHECK(
             password_wrapped_sync_key_envelope IS NULL
             OR length(password_wrapped_sync_key_envelope) BETWEEN 16 AND 131072
           ),
           remote_revision INTEGER CHECK(remote_revision IS NULL OR remote_revision >= 0),
           remote_etag TEXT CHECK(
             remote_etag IS NULL OR (
               length(CAST(remote_etag AS BLOB)) BETWEEN 2 AND 1024
               AND substr(remote_etag, 1, 1) = '\"'
               AND substr(remote_etag, -1, 1) = '\"'
               AND instr(substr(remote_etag, 2, length(remote_etag) - 2), '\"') = 0
             )
           ),
           baseline_content_sha256 TEXT CHECK(
             baseline_content_sha256 IS NULL OR (
               length(baseline_content_sha256) = 64
               AND baseline_content_sha256 = lower(baseline_content_sha256)
               AND baseline_content_sha256 NOT GLOB '*[^0-9a-f]*'
             )
           ),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id),
           CHECK(
             password_wrapped_sync_key_envelope IS NULL
             OR sync_key_secret_ref_id IS NOT NULL
           ),
           CHECK(
             (remote_revision IS NULL AND remote_etag IS NULL
                AND baseline_content_sha256 IS NULL)
             OR
             (remote_revision IS NOT NULL AND remote_etag IS NOT NULL
                AND baseline_content_sha256 IS NOT NULL)
           )
         ) STRICT;
         CREATE UNIQUE INDEX IF NOT EXISTS ssh_sync_profile_states_sync_key_ref
           ON ssh_sync_profile_states(sync_key_secret_ref_id)
           WHERE sync_key_secret_ref_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS ssh_sync_profile_states_plugin
           ON ssh_sync_profile_states(plugin_id, signer_fingerprint_sha256, profile_id);
         PRAGMA user_version = 22;
         COMMIT;",
    )?;
    Ok(())
}

/// Adds a digest for the locally retained encrypted baseline exchange and the
/// stable, owner-bound portable-to-local object identity map. Existing v22
/// baselines are cleared because their exchange digest cannot be reconstructed
/// safely from SQLite metadata alone.
fn migrate_v22_to_v23(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE ssh_sync_profile_states RENAME TO ssh_sync_profile_states_v22;
         CREATE TABLE ssh_sync_profile_states (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           scope_mode TEXT NOT NULL CHECK(scope_mode IN ('all_eligible', 'custom')),
           custom_host_ids_json TEXT NOT NULL CHECK(
             length(CAST(custom_host_ids_json AS BLOB)) BETWEEN 2 AND 131072
           ),
           custom_credential_ref_ids_json TEXT NOT NULL CHECK(
             length(CAST(custom_credential_ref_ids_json AS BLOB)) BETWEEN 2 AND 524288
           ),
           sync_key_secret_ref_id TEXT CHECK(
             sync_key_secret_ref_id IS NULL
             OR length(sync_key_secret_ref_id) BETWEEN 1 AND 160
           ),
           password_wrapped_sync_key_envelope BLOB CHECK(
             password_wrapped_sync_key_envelope IS NULL
             OR length(password_wrapped_sync_key_envelope) BETWEEN 16 AND 131072
           ),
           remote_revision INTEGER CHECK(remote_revision IS NULL OR remote_revision >= 0),
           remote_etag TEXT CHECK(
             remote_etag IS NULL OR (
               length(CAST(remote_etag AS BLOB)) BETWEEN 2 AND 1024
               AND substr(remote_etag, 1, 1) = '\"'
               AND substr(remote_etag, -1, 1) = '\"'
               AND instr(substr(remote_etag, 2, length(remote_etag) - 2), '\"') = 0
             )
           ),
           baseline_content_sha256 TEXT CHECK(
             baseline_content_sha256 IS NULL OR (
               length(baseline_content_sha256) = 64
               AND baseline_content_sha256 = lower(baseline_content_sha256)
               AND baseline_content_sha256 NOT GLOB '*[^0-9a-f]*'
             )
           ),
           baseline_exchange_sha256 TEXT CHECK(
             baseline_exchange_sha256 IS NULL OR (
               length(baseline_exchange_sha256) = 64
               AND baseline_exchange_sha256 = lower(baseline_exchange_sha256)
               AND baseline_exchange_sha256 NOT GLOB '*[^0-9a-f]*'
             )
           ),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id),
           CHECK(
             password_wrapped_sync_key_envelope IS NULL
             OR sync_key_secret_ref_id IS NOT NULL
           ),
           CHECK(
             (remote_revision IS NULL AND remote_etag IS NULL
                AND baseline_content_sha256 IS NULL AND baseline_exchange_sha256 IS NULL)
             OR
             (remote_revision IS NOT NULL AND remote_etag IS NOT NULL
                AND baseline_content_sha256 IS NOT NULL
                AND baseline_exchange_sha256 IS NOT NULL)
           )
         ) STRICT;
         INSERT INTO ssh_sync_profile_states
           (plugin_id, signer_fingerprint_sha256, profile_id, scope_mode,
            custom_host_ids_json, custom_credential_ref_ids_json,
            sync_key_secret_ref_id, password_wrapped_sync_key_envelope,
            remote_revision, remote_etag, baseline_content_sha256,
            baseline_exchange_sha256, state_version, updated_at_ms)
         SELECT plugin_id, signer_fingerprint_sha256, profile_id, scope_mode,
                custom_host_ids_json, custom_credential_ref_ids_json,
                sync_key_secret_ref_id, password_wrapped_sync_key_envelope,
                NULL, NULL, NULL, NULL, state_version, updated_at_ms
         FROM ssh_sync_profile_states_v22;
         DROP TABLE ssh_sync_profile_states_v22;
         CREATE UNIQUE INDEX ssh_sync_profile_states_sync_key_ref
           ON ssh_sync_profile_states(sync_key_secret_ref_id)
           WHERE sync_key_secret_ref_id IS NOT NULL;
         CREATE INDEX ssh_sync_profile_states_plugin
           ON ssh_sync_profile_states(plugin_id, signer_fingerprint_sha256, profile_id);
         CREATE TABLE IF NOT EXISTS ssh_sync_object_mappings (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           object_kind TEXT NOT NULL CHECK(
             object_kind IN ('host', 'identity', 'credential', 'secret')
           ),
           portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
           local_object_id TEXT NOT NULL CHECK(length(local_object_id) = 36),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_profile_states(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(
             plugin_id, signer_fingerprint_sha256, profile_id,
             object_kind, portable_object_id
           ),
           UNIQUE(
             plugin_id, signer_fingerprint_sha256, profile_id,
             object_kind, local_object_id
           )
         ) STRICT;
         CREATE INDEX IF NOT EXISTS ssh_sync_object_mappings_plugin
           ON ssh_sync_object_mappings(plugin_id, signer_fingerprint_sha256, profile_id);
         PRAGMA user_version = 23;
         COMMIT;",
    )?;
    Ok(())
}

fn migrate_v23_to_v24(connection: &Connection) -> Result<()> {
    let has_last_successful_sync = {
        let mut statement = connection.prepare("PRAGMA table_info(ssh_sync_profile_states)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        columns
            .iter()
            .any(|column| column == "last_successful_sync_at_ms")
    };
    let add_last_successful_sync = if has_last_successful_sync {
        ""
    } else {
        "ALTER TABLE ssh_sync_profile_states
           ADD COLUMN last_successful_sync_at_ms INTEGER
           CHECK(last_successful_sync_at_ms IS NULL OR last_successful_sync_at_ms >= 0);"
    };
    connection.execute_batch(&format!(
        "BEGIN IMMEDIATE;
         {add_last_successful_sync}
         CREATE TABLE IF NOT EXISTS ssh_sync_vault_gc_queue (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           secret_ref_id TEXT NOT NULL CHECK(length(secret_ref_id) = 36),
           reason TEXT NOT NULL CHECK(reason IN
             ('credential_replaced', 'credential_deleted', 'secret_replaced',
              'secret_deleted', 'host_updated')),
           attempt_id TEXT NOT NULL CHECK(length(attempt_id) = 36),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id)
         ) STRICT;
         CREATE INDEX IF NOT EXISTS ssh_sync_vault_gc_queue_plugin
           ON ssh_sync_vault_gc_queue(plugin_id, signer_fingerprint_sha256, profile_id);
         CREATE TABLE IF NOT EXISTS ssh_sync_owned_delta_operations (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           attempt_id TEXT NOT NULL CHECK(length(attempt_id) = 36),
           delta_sha256 TEXT NOT NULL CHECK(
             length(delta_sha256) = 64 AND delta_sha256 = lower(delta_sha256)
             AND delta_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           created_count INTEGER NOT NULL CHECK(created_count >= 0),
           updated_count INTEGER NOT NULL CHECK(updated_count >= 0),
           deleted_count INTEGER NOT NULL CHECK(deleted_count >= 0),
           gc_ref_ids_json TEXT NOT NULL CHECK(
             length(CAST(gc_ref_ids_json AS BLOB)) BETWEEN 2 AND 524288
           ),
           committed_at_ms INTEGER NOT NULL CHECK(committed_at_ms >= 0),
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id, attempt_id)
         ) STRICT;
         CREATE INDEX IF NOT EXISTS ssh_sync_owned_delta_operations_plugin
           ON ssh_sync_owned_delta_operations(plugin_id, signer_fingerprint_sha256, profile_id);
         PRAGMA user_version = 24;
         COMMIT;"
    ))?;
    Ok(())
}

/// Adds the durable fences required around plugin teardown, HTTP upload
/// retries, and profile-local portable scope membership.
fn migrate_v24_to_v25(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS ssh_sync_plugin_delete_operations (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           operation_id TEXT NOT NULL CHECK(length(operation_id) = 36),
           state TEXT NOT NULL CHECK(state IN ('pending', 'completed')),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= created_at_ms),
           PRIMARY KEY(plugin_id, operation_id)
         ) STRICT;
         CREATE UNIQUE INDEX IF NOT EXISTS ssh_sync_plugin_delete_one_pending
           ON ssh_sync_plugin_delete_operations(plugin_id)
           WHERE state = 'pending';
         CREATE TABLE IF NOT EXISTS ssh_sync_plugin_delete_profiles (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           operation_id TEXT NOT NULL CHECK(length(operation_id) = 36),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           FOREIGN KEY(plugin_id, operation_id)
             REFERENCES ssh_sync_plugin_delete_operations(plugin_id, operation_id)
             ON DELETE CASCADE,
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_profile_states(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id)
         ) STRICT;
         CREATE INDEX IF NOT EXISTS ssh_sync_plugin_delete_profiles_operation
           ON ssh_sync_plugin_delete_profiles(plugin_id, operation_id);
         CREATE TABLE IF NOT EXISTS ssh_sync_plugin_delete_refs (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           secret_ref_id TEXT NOT NULL CHECK(length(secret_ref_id) = 36),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_plugin_delete_profiles(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(
             plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id
           )
         ) STRICT;
         CREATE TABLE IF NOT EXISTS ssh_sync_http_upload_attempts (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           base_revision INTEGER NOT NULL CHECK(base_revision >= 0),
           base_etag TEXT,
           keyed_content_sha256 TEXT NOT NULL CHECK(
             length(keyed_content_sha256) = 64
             AND keyed_content_sha256 = lower(keyed_content_sha256)
             AND keyed_content_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           body_sha256 TEXT NOT NULL CHECK(
             length(body_sha256) = 64 AND body_sha256 = lower(body_sha256)
             AND body_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           idempotency_key TEXT NOT NULL CHECK(length(idempotency_key) = 36),
           state TEXT NOT NULL CHECK(state IN ('prepared', 'sent', 'verifying')),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= created_at_ms),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_profile_states(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id),
           UNIQUE(idempotency_key),
           CHECK(
             (base_revision = 0 AND base_etag IS NULL)
             OR
             (base_revision > 0 AND base_etag IS NOT NULL
               AND length(CAST(base_etag AS BLOB)) BETWEEN 2 AND 1024
               AND substr(base_etag, 1, 1) = '\"'
               AND substr(base_etag, -1, 1) = '\"'
               AND instr(substr(base_etag, 2, length(base_etag) - 2), '\"') = 0)
           )
         ) STRICT;
         CREATE TABLE IF NOT EXISTS ssh_sync_scope_memberships (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           object_kind TEXT NOT NULL CHECK(
             object_kind IN ('host', 'identity', 'credential', 'secret')
           ),
           portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
           membership TEXT NOT NULL CHECK(membership IN ('included', 'excluded')),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_profile_states(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(
             plugin_id, signer_fingerprint_sha256, profile_id,
             object_kind, portable_object_id
           )
         ) STRICT;
         CREATE TABLE IF NOT EXISTS ssh_sync_owned_delta_scope_replacements (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           attempt_id TEXT NOT NULL CHECK(length(attempt_id) = 36),
           memberships_json TEXT NOT NULL CHECK(
             length(CAST(memberships_json AS BLOB)) BETWEEN 2 AND 1048576
           ),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id, attempt_id)
             REFERENCES ssh_sync_owned_delta_operations(
               plugin_id, signer_fingerprint_sha256, profile_id, attempt_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id, attempt_id)
         ) STRICT;
         PRAGMA user_version = 25;
         COMMIT;",
    )?;
    Ok(())
}

/// Binds upload replay to its complete HTTP destination and removes the
/// historical global uniqueness constraint on SecretRef values. SecretRef is
/// an opaque handle to one Vault value and may intentionally be shared by
/// multiple typed consumers.
fn migrate_v25_to_v26(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         DROP TABLE ssh_sync_http_upload_attempts;
         CREATE TABLE ssh_sync_http_upload_attempts (
           plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(
             length(signer_fingerprint_sha256) = 64
             AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
             AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
           canonical_url TEXT NOT NULL CHECK(
             length(CAST(canonical_url AS BLOB)) BETWEEN 9 AND 2048
           ),
           http_method TEXT NOT NULL CHECK(http_method IN ('PUT', 'POST')),
           use_oauth INTEGER NOT NULL CHECK(use_oauth IN (0, 1)),
           authorization_revision INTEGER NOT NULL CHECK(authorization_revision >= 1),
           configuration_revision INTEGER NOT NULL CHECK(configuration_revision >= 1),
           base_revision INTEGER NOT NULL CHECK(base_revision >= 0),
           base_etag TEXT,
           target_revision INTEGER NOT NULL CHECK(target_revision > base_revision),
           keyed_content_sha256 TEXT NOT NULL CHECK(
             length(keyed_content_sha256) = 64
             AND keyed_content_sha256 = lower(keyed_content_sha256)
             AND keyed_content_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           body_sha256 TEXT NOT NULL CHECK(
             length(body_sha256) = 64 AND body_sha256 = lower(body_sha256)
             AND body_sha256 NOT GLOB '*[^0-9a-f]*'
           ),
           idempotency_key TEXT NOT NULL CHECK(length(idempotency_key) = 36),
           state TEXT NOT NULL CHECK(state IN ('prepared', 'sent', 'verifying')),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= created_at_ms),
           FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
             REFERENCES ssh_sync_profile_states(
               plugin_id, signer_fingerprint_sha256, profile_id
             ) ON DELETE CASCADE,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id),
           UNIQUE(idempotency_key),
           CHECK(
             (base_revision = 0 AND base_etag IS NULL)
             OR
             (base_revision > 0 AND base_etag IS NOT NULL
               AND length(CAST(base_etag AS BLOB)) BETWEEN 2 AND 1024
               AND substr(base_etag, 1, 1) = '\"'
               AND substr(base_etag, -1, 1) = '\"'
               AND instr(substr(base_etag, 2, length(base_etag) - 2), '\"') = 0)
           )
         ) STRICT;

         ALTER TABLE credential_secret_slots RENAME TO credential_secret_slots_v25;
         CREATE TABLE credential_secret_slots (
           credential_ref_id TEXT NOT NULL
             REFERENCES credential_refs(id) ON DELETE CASCADE,
           slot_index INTEGER NOT NULL CHECK(slot_index BETWEEN 0 AND 31),
           slot_kind TEXT NOT NULL CHECK(slot_kind IN
             ('password', 'private_key', 'passphrase', 'keyboard_interactive_answer')),
           secret_ref_id TEXT NOT NULL,
           label TEXT NOT NULL CHECK(length(label) BETWEEN 1 AND 120),
           PRIMARY KEY(credential_ref_id, slot_index)
         ) STRICT;
         INSERT INTO credential_secret_slots
           (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
         SELECT credential_ref_id, slot_index, slot_kind, secret_ref_id, label
         FROM credential_secret_slots_v25;
         DROP TABLE credential_secret_slots_v25;
         CREATE UNIQUE INDEX credential_secret_single_slot
           ON credential_secret_slots(credential_ref_id, slot_kind)
           WHERE slot_kind IN ('password', 'private_key', 'passphrase');
         CREATE TRIGGER credential_secret_slot_kind
           BEFORE INSERT ON credential_secret_slots
           WHEN NOT EXISTS(
             SELECT 1 FROM credential_refs
             WHERE id = NEW.credential_ref_id
               AND ((kind = 'password' AND NEW.slot_kind = 'password')
                 OR (kind = 'private_key' AND NEW.slot_kind IN ('private_key', 'passphrase'))
                 OR (kind = 'keyboard_interactive'
                   AND NEW.slot_kind = 'keyboard_interactive_answer'))
           )
           BEGIN SELECT RAISE(ABORT, 'credential secret slot kind mismatch'); END;
         PRAGMA user_version = 26;
         COMMIT;",
    )?;
    Ok(())
}

/// Extends the persisted whitelist without changing existing grant decisions or bindings.
fn migrate_v27_to_v28(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v27;
         CREATE TABLE plugin_capability_grants (
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
           major_version INTEGER NOT NULL CHECK(major_version >= 0),
           capability TEXT NOT NULL CHECK(capability IN
             ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
              'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
              'clipboard.write', 'terminal.metadata', 'terminal.observe',
              'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
              'host.metadata.read', 'host.mutation.propose', 'host.session.request',
              'remote.inspect', 'remote.exec.request',
              'network.domain', 'storage.plugin', 'sftp.read', 'sftp.write',
              'metrics.read', 'ssh.sync')),
           granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           updated_at_ms INTEGER NOT NULL,
           artifact_sha256 TEXT,
           app_version_major INTEGER,
           app_version_minor INTEGER,
           secure_surface_contract_revision INTEGER,
           PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
           CHECK(
             (artifact_sha256 IS NULL AND app_version_major IS NULL
               AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
             OR
             (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
               AND app_version_minor IS NOT NULL
               AND secure_surface_contract_revision IS NOT NULL
               AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
               AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
               AND app_version_major >= 0 AND app_version_minor >= 0
               AND secure_surface_contract_revision >= 1)
           )
         ) STRICT;
         INSERT INTO plugin_capability_grants
           (plugin_id, signer_fingerprint_sha256, major_version, capability,
            granted, state_version, updated_at_ms, artifact_sha256,
            app_version_major, app_version_minor, secure_surface_contract_revision)
         SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                granted, state_version, updated_at_ms, artifact_sha256,
                app_version_major, app_version_minor, secure_surface_contract_revision
         FROM plugin_capability_grants_v27;
         DROP TABLE plugin_capability_grants_v27;
         PRAGMA user_version = 28;
         COMMIT;",
    )?;
    Ok(())
}

/// Adds Core-owned package settings. There is deliberately no installation
/// foreign key: an uninstall may retain non-secret plugin data for a later,
/// explicitly revalidated installation of the same plugin id.
fn migrate_v28_to_v29(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE plugin_settings (
           plugin_id TEXT PRIMARY KEY,
           package_sha256 TEXT NOT NULL
             CHECK(length(package_sha256) = 64
               AND package_sha256 = lower(package_sha256)
               AND package_sha256 NOT GLOB '*[^0-9a-f]*'),
           schema_json TEXT NOT NULL CHECK(length(schema_json) BETWEEN 2 AND 32768),
           schema_sha256 TEXT NOT NULL
             CHECK(length(schema_sha256) = 64
               AND schema_sha256 = lower(schema_sha256)
               AND schema_sha256 NOT GLOB '*[^0-9a-f]*'),
           values_json TEXT NOT NULL CHECK(length(values_json) BETWEEN 2 AND 16384),
           revision INTEGER NOT NULL CHECK(revision >= 1),
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         PRAGMA user_version = 29;
         COMMIT;",
    )?;
    Ok(())
}

#[cfg(test)]
mod remote_capability_migration_tests {
    use super::*;

    #[test]
    fn version_35_preserves_v34_grants_and_keeps_credential_cleanup_metadata_after_uninstall() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE plugin_installations (plugin_id TEXT PRIMARY KEY) STRICT;
                 INSERT INTO plugin_installations VALUES ('org.example.fixture');
                 CREATE TABLE plugin_capability_grants (
                   plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
                   signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
                   major_version INTEGER NOT NULL,
                   capability TEXT NOT NULL CHECK(capability IN ('ui.panel')),
                   granted INTEGER NOT NULL, state_version INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL, artifact_sha256 TEXT,
                   app_version_major INTEGER, app_version_minor INTEGER,
                   secure_surface_contract_revision INTEGER,
                   PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
                 ) STRICT;
                 INSERT INTO plugin_capability_grants VALUES
                   ('org.example.fixture',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    1, 'ui.panel', 1, 4, 9,
                    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                    0, 1, 1);
                 CREATE TABLE plugin_operation_permissions (
                   permission_id TEXT PRIMARY KEY,
                   plugin_id TEXT NOT NULL,
                   signer_fingerprint_sha256 TEXT NOT NULL,
                   package_sha256 TEXT NOT NULL,
                   operation TEXT NOT NULL,
                   capability TEXT NOT NULL,
                   capability_major_version INTEGER NOT NULL,
                   capability_revision INTEGER NOT NULL,
                   artifact_sha256 TEXT NOT NULL,
                   app_version_major INTEGER NOT NULL,
                   app_version_minor INTEGER NOT NULL,
                   secure_surface_contract_revision INTEGER NOT NULL,
                   target_fingerprint BLOB NOT NULL,
                   target_label TEXT NOT NULL,
                   action_label TEXT NOT NULL,
                   created_at_ms INTEGER NOT NULL
                 ) STRICT;
                 CREATE INDEX plugin_operation_permissions_plugin_created
                   ON plugin_operation_permissions(plugin_id, created_at_ms, permission_id);
                 CREATE TABLE ssh_sync_profile_states (
                   plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
                   signer_fingerprint_sha256 TEXT NOT NULL CHECK(
                     length(signer_fingerprint_sha256) = 64
                     AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
                     AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
                   ),
                   profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
                   scope_mode TEXT NOT NULL CHECK(scope_mode IN ('all_eligible', 'custom')),
                   custom_host_ids_json TEXT NOT NULL CHECK(
                     length(CAST(custom_host_ids_json AS BLOB)) BETWEEN 2 AND 131072
                   ),
                   custom_credential_ref_ids_json TEXT NOT NULL CHECK(
                     length(CAST(custom_credential_ref_ids_json AS BLOB)) BETWEEN 2 AND 524288
                   ),
                   sync_key_secret_ref_id TEXT CHECK(
                     sync_key_secret_ref_id IS NULL
                     OR length(sync_key_secret_ref_id) BETWEEN 1 AND 160
                   ),
                   password_wrapped_sync_key_envelope BLOB CHECK(
                     password_wrapped_sync_key_envelope IS NULL
                     OR length(password_wrapped_sync_key_envelope) BETWEEN 16 AND 131072
                   ),
                   remote_revision INTEGER CHECK(remote_revision IS NULL OR remote_revision >= 0),
                   remote_etag TEXT,
                   baseline_content_sha256 TEXT,
                   baseline_exchange_sha256 TEXT,
                   state_version INTEGER NOT NULL CHECK(state_version >= 1),
                   updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
                   last_successful_sync_at_ms INTEGER
                     CHECK(last_successful_sync_at_ms IS NULL OR last_successful_sync_at_ms >= 0),
                   PRIMARY KEY(plugin_id, signer_fingerprint_sha256, profile_id)
                 ) STRICT;
                 CREATE TABLE ssh_sync_object_mappings (
                   plugin_id TEXT NOT NULL,
                   signer_fingerprint_sha256 TEXT NOT NULL,
                   profile_id TEXT NOT NULL,
                   object_kind TEXT NOT NULL CHECK(
                     object_kind IN ('host', 'identity', 'credential', 'secret')
                   ),
                   portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
                   local_object_id TEXT NOT NULL CHECK(length(local_object_id) = 36),
                   created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
                   updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
                   FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
                     REFERENCES ssh_sync_profile_states(
                       plugin_id, signer_fingerprint_sha256, profile_id
                     ) ON DELETE CASCADE,
                   PRIMARY KEY(
                     plugin_id, signer_fingerprint_sha256, profile_id,
                     object_kind, portable_object_id
                   ),
                   UNIQUE(
                     plugin_id, signer_fingerprint_sha256, profile_id,
                     object_kind, local_object_id
                   )
                 ) STRICT;
                 CREATE TABLE ssh_sync_scope_memberships (
                   plugin_id TEXT NOT NULL,
                   signer_fingerprint_sha256 TEXT NOT NULL,
                   profile_id TEXT NOT NULL,
                   object_kind TEXT NOT NULL CHECK(
                     object_kind IN ('host', 'identity', 'credential', 'secret')
                   ),
                   portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
                   membership TEXT NOT NULL CHECK(membership IN ('included', 'excluded')),
                   updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
                   FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
                     REFERENCES ssh_sync_profile_states(
                       plugin_id, signer_fingerprint_sha256, profile_id
                     ) ON DELETE CASCADE,
                   PRIMARY KEY(
                     plugin_id, signer_fingerprint_sha256, profile_id,
                     object_kind, portable_object_id
                   )
                 ) STRICT;
                 PRAGMA user_version = 34;",
            )
            .unwrap();

        migrate_v34_to_v35(&connection).unwrap();
        let preserved: (String, i64) = connection
            .query_row(
                "SELECT capability, state_version FROM plugin_capability_grants",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(preserved, ("ui.panel".to_owned(), 4));
        connection
            .execute(
                "INSERT INTO plugin_capability_grants VALUES
                 ('org.example.fixture',
                  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                  1, 'credentials.plugin', 1, 1, 10,
                  'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                  0, 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO plugin_credentials VALUES
                 ('00000000-0000-4000-8000-000000000001', 'org.example.fixture',
                  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                  '00000000-0000-4000-8000-000000000002', 'operation', 'idempotency',
                  'Token', 'https://api.example.test', 'bearer', NULL, 1,
                  'cleanup_pending', 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute("DELETE FROM plugin_installations", [])
            .unwrap();
        let cleanup_rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM plugin_credentials", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(cleanup_rows, 1);
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 35);
    }

    #[test]
    fn version_28_preserves_grant_bindings_and_accepts_remote_decisions() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
             CREATE TABLE plugin_installations (plugin_id TEXT PRIMARY KEY) STRICT;
             INSERT INTO plugin_installations VALUES ('org.example.fixture');
             CREATE TABLE plugin_capability_grants (
               plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
               signer_fingerprint_sha256 TEXT NOT NULL,
               major_version INTEGER NOT NULL,
               capability TEXT NOT NULL CHECK(capability IN ('ui.panel', 'clipboard.write')),
               granted INTEGER NOT NULL, state_version INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL, artifact_sha256 TEXT,
               app_version_major INTEGER, app_version_minor INTEGER,
               secure_surface_contract_revision INTEGER,
               PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
             ) STRICT;
             PRAGMA user_version = 27;",
            )
            .unwrap();
        let signer = "a".repeat(64);
        let artifact = "b".repeat(64);
        connection
            .execute(
                "INSERT INTO plugin_capability_grants VALUES
             ('org.example.fixture', ?1, 1, 'ui.panel', 1, 9, 123, ?2, 0, 1, 1),
             ('org.example.fixture', ?1, 1, 'clipboard.write', 0, 9, 123, NULL, NULL, NULL, NULL)",
                rusqlite::params![signer, artifact],
            )
            .unwrap();
        migrate_v27_to_v28(&connection).unwrap();
        let preserved: (i64, i64, i64, String, i64, i64, i64) = connection
            .query_row(
                "SELECT granted,state_version,updated_at_ms,artifact_sha256,
                    app_version_major,app_version_minor,secure_surface_contract_revision
             FROM plugin_capability_grants WHERE capability='ui.panel'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(preserved, (1, 9, 123, artifact.clone(), 0, 1, 1));
        let unbound: (i64, Option<String>) = connection.query_row(
            "SELECT granted,artifact_sha256 FROM plugin_capability_grants WHERE capability='clipboard.write'",
            [], |r| Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        assert_eq!(unbound, (0, None));
        for (capability, granted) in [("remote.inspect", 1), ("remote.exec.request", 0)] {
            connection
                .execute(
                    "INSERT INTO plugin_capability_grants VALUES
                 ('org.example.fixture',?1,1,?2,?3,1,124,?4,0,1,1)",
                    rusqlite::params![signer, capability, granted, artifact],
                )
                .unwrap();
        }
        assert!(
            connection
                .execute(
                    "INSERT INTO plugin_capability_grants VALUES
             ('org.example.fixture',?1,1,'remote.arbitrary',1,1,124,NULL,NULL,NULL,NULL)",
                    [&signer],
                )
                .is_err()
        );
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 28);
        connection
            .execute("DELETE FROM plugin_installations", [])
            .unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM plugin_capability_grants", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn version_29_adds_empty_settings_without_changing_v28_permission_data() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE plugin_installations (plugin_id TEXT PRIMARY KEY) STRICT;
                 INSERT INTO plugin_installations VALUES ('org.example.settings-fixture');
                 CREATE TABLE plugin_capability_grants (
                   plugin_id TEXT NOT NULL
                     REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
                   signer_fingerprint_sha256 TEXT NOT NULL,
                   major_version INTEGER NOT NULL,
                   capability TEXT NOT NULL CHECK(capability IN ('ui.panel')),
                   granted INTEGER NOT NULL,
                   state_version INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL,
                   artifact_sha256 TEXT,
                   app_version_major INTEGER,
                   app_version_minor INTEGER,
                   secure_surface_contract_revision INTEGER,
                   PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
                 ) STRICT;
                 PRAGMA user_version = 27;",
            )
            .unwrap();
        let signer = "a".repeat(64);
        let artifact = "b".repeat(64);
        connection
            .execute(
                "INSERT INTO plugin_capability_grants VALUES
                 ('org.example.settings-fixture', ?1, 1, 'ui.panel', 1, 9, 123,
                  ?2, 0, 1, 1)",
                rusqlite::params![signer, artifact],
            )
            .unwrap();

        migrate_v27_to_v28(&connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 28);
        let before: (String, i64, i64, String) = connection
            .query_row(
                "SELECT plugin_id, granted, state_version, artifact_sha256
                 FROM plugin_capability_grants WHERE capability = 'ui.panel'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        migrate_v28_to_v29(&connection).unwrap();

        let after: (String, i64, i64, String) = connection
            .query_row(
                "SELECT plugin_id, granted, state_version, artifact_sha256
                 FROM plugin_capability_grants WHERE capability = 'ui.panel'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(after, before);
        let installation_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM plugin_installations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(installation_count, 1);
        let settings_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM plugin_settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(settings_count, 0);
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 29);
    }
}
