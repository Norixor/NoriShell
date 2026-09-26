//! Provider policy over non-secret descriptors returned by the data broker.

use std::collections::{BTreeMap, BTreeSet};

pub const CATEGORIES: [&str; 3] = ["hosts", "credentials", "desktopProfiles"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub handle: String,
    pub equality_tag: String,
    pub updated_at: Option<i64>,
    pub deleted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Prompt,
    Newest,
}

impl Policy {
    pub fn parse(value: &str) -> Self {
        if value == "newest" {
            Self::Newest
        } else {
            Self::Prompt
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub objects: Vec<Chosen>,
    pub deletions: Vec<Chosen>,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Local,
    Remote,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Chosen {
    pub source: Source,
    pub handle: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InvalidSnapshot {
    DuplicateObject,
    MissingIdentity,
}

fn indexed(items: &[Item]) -> Result<BTreeMap<&str, &Item>, InvalidSnapshot> {
    let mut result = BTreeMap::new();
    for item in items {
        if item.id.is_empty() || item.handle.is_empty() || item.equality_tag.is_empty() {
            return Err(InvalidSnapshot::MissingIdentity);
        }
        if result.insert(item.id.as_str(), item).is_some() {
            return Err(InvalidSnapshot::DuplicateObject);
        }
    }
    Ok(result)
}

fn equal(left: &Item, right: &Item) -> bool {
    left.deleted == right.deleted && left.equality_tag == right.equality_tag
}

/// Missing entries never imply deletion. Only authenticated tombstones can delete.
/// Equal or absent clocks cannot resolve a concurrent edit under the newest policy.
pub fn select(
    local: &[Item],
    remote: &[Item],
    baseline: &[Item],
    conflict_policy: Policy,
    deletion_policy: Policy,
) -> Result<Selection, InvalidSnapshot> {
    let local = indexed(local)?;
    let remote = indexed(remote)?;
    let baseline = indexed(baseline)?;
    let ids: BTreeSet<_> = local.keys().chain(remote.keys()).copied().collect();
    let mut selection = Selection::default();
    for id in ids {
        let selected = match (local.get(id).copied(), remote.get(id).copied()) {
            (Some(left), Some(right)) if equal(left, right) => Some((left, Source::Local)),
            (Some(left), Some(right)) => {
                let base = baseline.get(id).copied();
                if base.is_some_and(|base| equal(left, base)) {
                    Some((right, Source::Remote))
                } else if base.is_some_and(|base| equal(right, base)) {
                    Some((left, Source::Local))
                } else if conflict_policy == Policy::Newest {
                    match (left.updated_at, right.updated_at) {
                        (Some(left_time), Some(right_time)) if left_time > right_time => {
                            Some((left, Source::Local))
                        }
                        (Some(left_time), Some(right_time)) if right_time > left_time => {
                            Some((right, Source::Remote))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            (Some(item), None) => Some((item, Source::Local)),
            (None, Some(item)) => Some((item, Source::Remote)),
            (None, None) => unreachable!(),
        };
        match selected {
            Some((item, source)) if item.deleted => {
                let other = match source {
                    Source::Local => remote.get(id),
                    Source::Remote => local.get(id),
                };
                let unchanged_deletion = other.is_none_or(|other| other.deleted)
                    || baseline.get(id).is_some_and(|base| equal(base, item));
                if deletion_policy == Policy::Prompt && !unchanged_deletion {
                    selection.conflicts.push(id.to_owned());
                } else {
                    selection.deletions.push(Chosen {
                        source,
                        handle: item.handle.clone(),
                    });
                }
            }
            Some((item, source)) => selection.objects.push(Chosen {
                source,
                handle: item.handle.clone(),
            }),
            None => selection.conflicts.push(id.to_owned()),
        }
    }
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(handle: &str, tag: &str, time: Option<i64>, deleted: bool) -> Item {
        Item {
            id: "host:one".into(),
            handle: handle.into(),
            equality_tag: tag.into(),
            updated_at: time,
            deleted,
        }
    }

    #[test]
    fn unchanged_local_accepts_remote_edit_without_clock_guessing() {
        let local = item("local", "old", None, false);
        let remote = item("remote", "new", None, false);
        let result = select(
            std::slice::from_ref(&local),
            &[remote],
            std::slice::from_ref(&local),
            Policy::Prompt,
            Policy::Prompt,
        )
        .unwrap();
        assert_eq!(
            result.objects,
            [Chosen {
                source: Source::Remote,
                handle: "remote".into()
            }]
        );
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn missing_remote_does_not_delete_local() {
        let local = item("local", "old", Some(1), false);
        let result = select(
            std::slice::from_ref(&local),
            &[],
            std::slice::from_ref(&local),
            Policy::Newest,
            Policy::Newest,
        )
        .unwrap();
        assert_eq!(
            result.objects,
            [Chosen {
                source: Source::Local,
                handle: "local".into()
            }]
        );
        assert!(result.deletions.is_empty());
    }

    #[test]
    fn equal_or_unknown_clocks_require_review() {
        for remote_time in [None, Some(10)] {
            let result = select(
                &[item("local", "a", Some(10), false)],
                &[item("remote", "b", remote_time, false)],
                &[],
                Policy::Newest,
                Policy::Newest,
            )
            .unwrap();
            assert_eq!(result.conflicts, ["host:one"]);
            assert!(result.objects.is_empty());
        }
    }

    #[test]
    fn deletion_policy_remains_independent_of_conflict_policy() {
        let local = item("local", "a", Some(10), false);
        let remote = item("remote", "deleted", Some(11), true);
        let result = select(&[local], &[remote], &[], Policy::Newest, Policy::Prompt).unwrap();
        assert_eq!(result.conflicts, ["host:one"]);
        assert!(result.deletions.is_empty());
    }
}
