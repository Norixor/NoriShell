//! Bounded, non-secret display cache. It never stores object handles or sync authority.

use serde_json::{Value, json};

const MAX_ROWS: usize = 128;

#[derive(Default)]
pub struct BrowserCache {
    pub rows: Vec<Value>,
    pub stale: bool,
    pub omitted: usize,
    pub verified_at_unix_ms: Option<u64>,
    counts: Option<[u64; 3]>,
}

fn display_text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .take(96)
        .collect()
}

impl BrowserCache {
    pub fn restore(&mut self, storage: &Value, origin: Option<&str>) {
        let Some(encoded) = storage.get("valueJson").and_then(Value::as_str) else {
            return;
        };
        if encoded.len() > 64 * 1024 {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(encoded) else {
            return;
        };
        if !matches!(value["schema"].as_u64(), Some(1..=3))
            || value["origin"].as_str() != origin
            || origin.is_none()
        {
            return;
        }
        let Some(rows) = value["rows"].as_array() else {
            return;
        };
        self.replace(rows, value["verifiedAtUnixMs"].as_u64());
        self.omitted = value["omitted"]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0);
        self.counts = if value["schema"] == 3 {
            match (
                value["counts"]["hosts"].as_u64(),
                value["counts"]["credentials"].as_u64(),
                value["counts"]["desktopProfiles"].as_u64(),
            ) {
                (Some(hosts), Some(credentials), Some(desktops)) => {
                    Some([hosts, credentials, desktops])
                }
                _ => None,
            }
        } else if self.omitted == 0 {
            self.counts
        } else {
            None
        };
        self.stale = true;
    }

    pub fn replace(&mut self, rows: &[Value], verified_at_unix_ms: Option<u64>) {
        self.counts = Some([
            Self::count_rows(rows, "hosts"),
            Self::count_rows(rows, "credentials"),
            Self::count_rows(rows, "desktopProfiles"),
        ]);
        self.rows = rows
            .iter()
            .filter_map(|row| {
                let category = row["category"].as_str()?;
                if !super::sync_policy::CATEGORIES.contains(&category) || row["tombstone"] == true {
                    return None;
                }
                Some(json!({
                    "category":category,
                    "label":display_text(row, "label"),
                    "address":display_text(row, "address"),
                    "username":display_text(row, "username"),
                    "materialKind":display_text(row, "materialKind")
                }))
            })
            .take(MAX_ROWS)
            .collect();
        self.omitted = rows.len().saturating_sub(self.rows.len());
        self.verified_at_unix_ms = verified_at_unix_ms;
        self.stale = false;
    }

    pub fn stored(&self, origin: &str) -> String {
        json!({"schema":3,"origin":origin,"rows":self.rows,"omitted":self.omitted,
        "verifiedAtUnixMs":self.verified_at_unix_ms,
        "counts":self.counts.map(|counts| json!({
            "hosts":counts[0],"credentials":counts[1],"desktopProfiles":counts[2]
        }))})
        .to_string()
    }

    fn count_rows(rows: &[Value], category: &str) -> u64 {
        rows.iter()
            .filter(|row| row["category"] == category && row["tombstone"] != true)
            .count() as u64
    }

    pub fn category_count(&self, category: &str) -> Option<u64> {
        let counts = self.counts?;
        match category {
            "hosts" => Some(counts[0]),
            "credentials" => Some(counts[1]),
            "desktopProfiles" => Some(counts[2]),
            _ => None,
        }
    }

    pub fn verified_at_utc(&self) -> Option<String> {
        let seconds = i64::try_from(self.verified_at_unix_ms? / 1_000).ok()?;
        let days = seconds.div_euclid(86_400);
        let day_seconds = seconds.rem_euclid(86_400);
        // Civil date from a Unix day count; output UTC explicitly because the
        // isolated Wasm guest has no access to the host's local timezone.
        let z = days.checked_add(719_468)?;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let mut year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        if month <= 2 {
            year += 1;
        }
        if !(1970..=9999).contains(&year) {
            return None;
        }
        Some(format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
            day_seconds / 3_600,
            day_seconds % 3_600 / 60
        ))
    }

    pub fn table(
        &self,
        category: &str,
        title: &str,
        name: &str,
        target: &str,
        empty: &str,
    ) -> Value {
        let rows: Vec<Value> = self.rows.iter().filter(|row| row["category"] == category)
            .enumerate().map(|(index, row)| json!({
                "rowId":format!("row{index}"),
                "cells":[row["label"], if category == "credentials" { row["materialKind"].clone() } else { row["address"].clone() }],
                "actionId":null
            })).collect();
        json!({"kind":"table","nodeId":format!("data{category}"),"label":title,
            "columns":[{"columnId":"name","label":name,"width":null},{"columnId":"target","label":target,"width":null}],
            "rows":rows,"emptyText":empty})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_excludes_authority_and_secrets_and_is_origin_bound() {
        let mut cache = BrowserCache::default();
        cache.replace(&[json!({"category":"credentials","label":"Login","materialKind":"password",
            "password":"secret","objectHandle":"authority","equalityTag":"digest","secretRef":"private"})], Some(1_790_429_340_000));
        let saved = cache.stored("https://one.example");
        for forbidden in ["secret", "authority", "digest", "private"] {
            assert!(!saved.contains(forbidden));
        }
        let mut restored = BrowserCache::default();
        restored.restore(&json!({"valueJson":saved}), Some("https://two.example"));
        assert!(restored.rows.is_empty());
        restored.restore(&json!({"valueJson":saved}), Some("https://one.example"));
        assert_eq!(restored.rows.len(), 1);
        assert!(restored.stale);
        assert_eq!(restored.category_count("credentials"), Some(1));
        assert_eq!(restored.verified_at_unix_ms, cache.verified_at_unix_ms);
        assert_eq!(
            restored.verified_at_utc().as_deref(),
            Some("2026-09-26 13:29 UTC")
        );
    }

    #[test]
    fn verified_time_handles_leap_day_and_invalid_clock() {
        let mut cache = BrowserCache {
            verified_at_unix_ms: Some(1_709_251_140_000),
            ..Default::default()
        };
        assert_eq!(
            cache.verified_at_utc().as_deref(),
            Some("2024-02-29 23:59 UTC")
        );
        cache.verified_at_unix_ms = Some(u64::MAX);
        assert!(cache.verified_at_utc().is_none());
    }

    #[test]
    fn full_counts_survive_bounded_rows_and_legacy_cache_does_not_guess() {
        let rows: Vec<Value> = (0..130)
            .map(|index| {
                json!({
                    "category":"hosts","label":format!("Host {index}")
                })
            })
            .collect();
        let mut cache = BrowserCache::default();
        cache.replace(&rows, Some(1_790_429_340_000));
        assert_eq!(cache.rows.len(), MAX_ROWS);
        assert_eq!(cache.category_count("hosts"), Some(130));
        let mut restored = BrowserCache::default();
        restored.restore(
            &json!({"valueJson":cache.stored("https://one.example")}),
            Some("https://one.example"),
        );
        assert_eq!(restored.category_count("hosts"), Some(130));

        let mut legacy =
            serde_json::from_str::<Value>(&cache.stored("https://one.example")).unwrap();
        legacy["schema"] = json!(2);
        legacy.as_object_mut().unwrap().remove("counts");
        restored.restore(
            &json!({"valueJson":legacy.to_string()}),
            Some("https://one.example"),
        );
        assert_eq!(restored.category_count("hosts"), None);
    }
}
