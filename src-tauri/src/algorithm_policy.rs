use norishell_core_api::{
    AlgorithmCatalogEntry, AlgorithmCategory, AlgorithmCompatibilityException,
    AlgorithmPolicyCatalog, AlgorithmRisk,
};
use norishell_ssh_transport::{
    ALGORITHM_POLICY_CATALOG_VERSION, AlgorithmCategory as TransportAlgorithmCategory,
    AlgorithmPolicy, AlgorithmRisk as TransportAlgorithmRisk, SECURE_DEFAULT_ALGORITHM_POLICY_ID,
    algorithm_policy_catalog,
};

pub(crate) fn catalog() -> AlgorithmPolicyCatalog {
    AlgorithmPolicyCatalog {
        catalog_version: ALGORITHM_POLICY_CATALOG_VERSION.to_owned(),
        default_policy_id: SECURE_DEFAULT_ALGORITHM_POLICY_ID.to_owned(),
        entries: algorithm_policy_catalog()
            .iter()
            .map(|entry| AlgorithmCatalogEntry {
                stable_id: entry.stable_id.to_owned(),
                category: category_to_wire(entry.category),
                algorithm_name: entry.algorithm_name.to_owned(),
                available: true,
                enabled_by_default: entry.enabled_by_default,
                selectable_exception: entry.selectable_exception,
                risk: risk_to_wire(entry.risk),
                risk_message_key: match entry.risk {
                    TransportAlgorithmRisk::Modern => None,
                    TransportAlgorithmRisk::Legacy => {
                        Some("sshHosts.algorithms.riskLegacy".to_owned())
                    }
                    TransportAlgorithmRisk::Weak => Some("sshHosts.algorithms.riskWeak".to_owned()),
                },
            })
            .collect(),
    }
}

pub(crate) fn resolve(
    policy_id: &str,
    compatibility_exceptions: &[AlgorithmCompatibilityException],
) -> Result<AlgorithmPolicy, ()> {
    AlgorithmPolicy::from_catalog_ids(
        policy_id,
        compatibility_exceptions.iter().map(|exception| {
            (
                category_from_wire(exception.category),
                exception.exception_id.as_str(),
            )
        }),
    )
    .map_err(|_| ())
}

const fn category_to_wire(category: TransportAlgorithmCategory) -> AlgorithmCategory {
    match category {
        TransportAlgorithmCategory::KeyExchange => AlgorithmCategory::KeyExchange,
        TransportAlgorithmCategory::HostKey => AlgorithmCategory::HostKey,
        TransportAlgorithmCategory::Cipher => AlgorithmCategory::Cipher,
        TransportAlgorithmCategory::Mac => AlgorithmCategory::Mac,
    }
}

const fn category_from_wire(category: AlgorithmCategory) -> TransportAlgorithmCategory {
    match category {
        AlgorithmCategory::KeyExchange => TransportAlgorithmCategory::KeyExchange,
        AlgorithmCategory::HostKey => TransportAlgorithmCategory::HostKey,
        AlgorithmCategory::Cipher => TransportAlgorithmCategory::Cipher,
        AlgorithmCategory::Mac => TransportAlgorithmCategory::Mac,
    }
}

const fn risk_to_wire(risk: TransportAlgorithmRisk) -> AlgorithmRisk {
    match risk {
        TransportAlgorithmRisk::Modern => AlgorithmRisk::Modern,
        TransportAlgorithmRisk::Legacy => AlgorithmRisk::Legacy,
        TransportAlgorithmRisk::Weak => AlgorithmRisk::Weak,
    }
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{AlgorithmCategory, AlgorithmCompatibilityException};

    use super::{catalog, resolve};

    #[test]
    fn public_catalog_has_unique_ids_and_never_exposes_none() {
        let catalog = catalog();
        let mut ids = std::collections::HashSet::new();
        assert!(
            catalog
                .entries
                .iter()
                .all(|entry| ids.insert(&entry.stable_id))
        );
        assert!(
            catalog
                .entries
                .iter()
                .all(|entry| entry.algorithm_name != "none")
        );
        assert!(
            catalog
                .entries
                .iter()
                .filter(|entry| entry.selectable_exception)
                .all(|entry| entry.available && !entry.enabled_by_default)
        );
    }

    #[test]
    fn wire_policy_resolution_rejects_unknown_and_category_mismatch() {
        let unknown = AlgorithmCompatibilityException {
            category: AlgorithmCategory::Mac,
            exception_id: "compat-mac-not-real".to_owned(),
            reason: None,
        };
        assert!(resolve("secure-default", &[unknown]).is_err());

        let mismatch = AlgorithmCompatibilityException {
            category: AlgorithmCategory::Cipher,
            exception_id: "compat-kex-dh-group14-sha1".to_owned(),
            reason: None,
        };
        assert!(resolve("secure-default", &[mismatch]).is_err());
    }
}
