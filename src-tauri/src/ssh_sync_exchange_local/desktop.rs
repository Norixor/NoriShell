use super::*;

impl NoriShellSshSyncLocalAdapter {
    pub(super) fn desktop_profiles(&self) -> Result<Vec<DesktopProfile>, PortableStoreError> {
        self.hosts
            .with_ssh_sync_repository(|r| r.list_desktop_profiles())
            .map_err(|error| map_store_error_at("local.desktop_profiles", error))
    }

    pub(super) fn desktop_password(
        &self,
        id: &CredentialRefId,
    ) -> Result<CredentialRecord, PortableStoreError> {
        let record = self
            .hosts
            .with_ssh_sync_repository(|r| r.get_ready_credential_record(id))
            .map_err(|error| map_store_error_at("local.desktop_password", error))?;
        if !matches!(record.details, CredentialRecordDetails::Password { .. }) {
            return Err(PortableStoreError::Rejected("desktop.desktop_password.01"));
        }
        Ok(record)
    }

    pub(super) fn selection_desktops(
        &self,
    ) -> Result<Vec<SshSyncSecureDesktopProfile>, PortableStoreError> {
        self.desktop_profiles()?
            .into_iter()
            .map(|profile| {
                let credentials = profile
                    .credential_ref_id
                    .as_ref()
                    .map(|id| {
                        self.desktop_password(id)
                            .map(|record| SshSyncSecureCredential {
                                credential_ref_id: record.credential_ref_id,
                                label: record.label,
                                method_label: authentication_method_label(
                                    AuthenticationMethodKind::Password,
                                )
                                .to_owned(),
                                machine_bound: false,
                            })
                    })
                    .transpose()?
                    .into_iter()
                    .collect();
                Ok(SshSyncSecureDesktopProfile {
                    profile_id: profile.id,
                    label: profile.label,
                    protocol: profile.protocol,
                    address: profile.address,
                    port: profile.port,
                    username: profile.username,
                    domain: profile.domain,
                    credentials,
                })
            })
            .collect()
    }
}

pub(super) fn mapped_desktop_id(mappings: &LocalIdMap, id: PortableObjectId) -> Option<String> {
    match mappings.get(&(SshSyncObjectKind::DesktopProfile, id)) {
        Some(SshSyncLocalObjectId::DesktopProfile(value)) => Some(value.clone()),
        _ => None,
    }
}
