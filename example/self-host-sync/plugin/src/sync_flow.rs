//! The provider's sync decisions; Core only executes the requested primitives.

use serde_json::{Value, json};

use crate::{
    network_flow::{NetworkFlow, Progress, Receipt},
    sync_policy::{self, Item, Policy, Source},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Refresh,
    Sync,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Snapshot,
    Download,
    Inspect,
    Compose,
    Export,
    Upload,
    Apply,
    Checkpoint,
}

pub struct Flow {
    pub intent: Intent,
    pub phase: Phase,
    pub local: Value,
    pub remote: Value,
    pub composed: Value,
    pub exported: Value,
    pub applied: Value,
    pub download: Option<Receipt>,
    pub upload: Option<Receipt>,
    pub network: NetworkFlow,
    pub url: String,
    pub conflict_policy: Policy,
    pub deletion_policy: Policy,
    pub choice: Option<Source>,
    state_handles: Vec<String>,
    blob_handles: Vec<String>,
    receipt_handles: Vec<String>,
}

pub enum Transition {
    Call(Value),
    Finished {
        difference: &'static str,
        review: bool,
    },
    Failed {
        code: String,
        http_status: Option<u16>,
    },
}

impl Flow {
    pub fn new(
        intent: Intent,
        url: String,
        conflict: &str,
        deletion: &str,
        choice: Option<Source>,
    ) -> Self {
        Self {
            intent,
            phase: Phase::Snapshot,
            local: Value::Null,
            remote: Value::Null,
            composed: Value::Null,
            exported: Value::Null,
            applied: Value::Null,
            download: None,
            upload: None,
            network: NetworkFlow::default(),
            url,
            conflict_policy: Policy::parse(conflict),
            deletion_policy: Policy::parse(deletion),
            choice,
            state_handles: Vec::new(),
            blob_handles: Vec::new(),
            receipt_handles: Vec::new(),
        }
    }

    pub fn release_request(&self) -> Value {
        json!({"kind":"dataRelease","request":{"profileId":"primary",
            "stateHandles":self.state_handles,"blobHandles":self.blob_handles,
            "receiptHandles":self.receipt_handles}})
    }

    pub fn snapshot_request() -> Value {
        json!({"kind":"dataSnapshot","request":{"profileId":"primary","categories":sync_policy::CATEGORIES}})
    }

    pub fn receive_snapshot(&mut self, value: Value) -> Transition {
        if value["kind"] != "dataSnapshot"
            || value["snapshotHandle"].as_str().is_none()
            || !value["objects"].is_array()
        {
            return invalid_response();
        }
        self.record_state(&value, "snapshotHandle");
        self.local = value;
        if self.remote["inspectionHandle"].is_string() {
            return self.decide();
        }
        self.phase = Phase::Download;
        Transition::Call(NetworkFlow::start(&self.url, "get", vec![], None))
    }

    pub fn receive_download(&mut self, value: &Value) -> Transition {
        match self.network.receive(value) {
            Progress::Call(call) => Transition::Call(call),
            Progress::Failed(code) => Transition::Failed {
                code,
                http_status: None,
            },
            Progress::Complete(receipt) => {
                self.receipt_handles.push(receipt.handle.clone());
                if let Some(blob) = &receipt.blob {
                    self.blob_handles.push(blob.clone());
                }
                if receipt.status == 200 {
                    let Some(blob) = receipt.blob.as_deref() else {
                        return invalid_response();
                    };
                    let request = json!({"kind":"dataInspect","request":{
                        "profileId":"primary","categories":sync_policy::CATEGORIES,
                        "receiptHandle":receipt.handle,"bodyBlobHandle":blob
                    }});
                    self.download = Some(receipt);
                    self.phase = Phase::Inspect;
                    Transition::Call(request)
                } else if receipt.status == 404 {
                    self.download = Some(receipt);
                    self.remote = json!({"objects":[]});
                    if self.intent == Intent::Refresh {
                        Transition::Finished {
                            difference: if objects_empty(&self.local) {
                                "equal"
                            } else {
                                "localOnly"
                            },
                            review: false,
                        }
                    } else {
                        self.export()
                    }
                } else {
                    http_failure(receipt.status)
                }
            }
        }
    }

    pub fn receive_inspection(&mut self, value: Value) -> Transition {
        if value["kind"] != "dataInspect"
            || value["inspectionHandle"].as_str().is_none()
            || items(&value).is_err()
        {
            return invalid_response();
        }
        self.record_state(&value, "inspectionHandle");
        self.remote = value;
        // Remote key recovery may change the keyed descriptor namespace on a
        // new device. Re-read local objects only after authentication succeeds.
        self.phase = Phase::Snapshot;
        Transition::Call(Self::snapshot_request())
    }

    fn decide(&mut self) -> Transition {
        let Ok(local) = items(&self.local) else {
            return invalid_response();
        };
        let Ok(remote) = items(&self.remote) else {
            return invalid_response();
        };
        let equal = same_content(&local, &remote);
        if self.intent == Intent::Refresh {
            return Transition::Finished {
                difference: if equal { "equal" } else { "different" },
                review: false,
            };
        }
        if equal && self.remote["migrationRequired"] != true {
            self.phase = Phase::Checkpoint;
            let Some(download) = &self.download else {
                return invalid_response();
            };
            return Transition::Call(json!({"kind":"dataCheckpoint","request":{
                "profileId":"primary","categories":sync_policy::CATEGORIES,
                "sourceHandle":self.local["snapshotHandle"],
                "expectedLocalSnapshotHandle":self.local["snapshotHandle"],
                "remoteInspectionHandle":self.remote["inspectionHandle"],
                "authoritativeReceiptHandle":download.handle}}));
        }
        self.compose()
    }

    fn compose(&mut self) -> Transition {
        let Ok(local) = items(&self.local) else {
            return invalid_response();
        };
        let Ok(remote) = items(&self.remote) else {
            return invalid_response();
        };
        let Ok(mut selection) = sync_policy::select(
            &local,
            &remote,
            &[],
            self.conflict_policy,
            self.deletion_policy,
        ) else {
            return invalid_response();
        };
        if !selection.conflicts.is_empty() {
            let Some(source) = self.choice else {
                return Transition::Finished {
                    difference: "conflict",
                    review: true,
                };
            };
            for id in &selection.conflicts {
                let selected = match source {
                    Source::Local => local.iter(),
                    Source::Remote => remote.iter(),
                }
                .find(|item| &item.id == id);
                let Some(item) = selected else {
                    return Transition::Finished {
                        difference: "conflict",
                        review: true,
                    };
                };
                let chosen = sync_policy::Chosen {
                    source,
                    handle: item.handle.clone(),
                };
                if item.deleted {
                    selection.deletions.push(chosen)
                } else {
                    selection.objects.push(chosen)
                }
            }
        }
        let decisions: Vec<Value> = selection.objects.iter().chain(&selection.deletions).map(|chosen| json!({
            "objectHandle":chosen.handle,"source":match chosen.source {Source::Local=>"local",Source::Remote=>"remote"}
        })).collect();
        self.phase = Phase::Compose;
        Transition::Call(
            json!({"kind":"dataCompose","request":{"profileId":"primary",
            "categories":sync_policy::CATEGORIES,"localSnapshotHandle":self.local["snapshotHandle"],
            "remoteInspectionHandle":self.remote["inspectionHandle"],"decisions":decisions}}),
        )
    }

    pub fn receive(&mut self, value: Value) -> Transition {
        match self.phase {
            Phase::Snapshot => self.receive_snapshot(value),
            Phase::Download => self.receive_download(&value),
            Phase::Inspect => self.receive_inspection(value),
            Phase::Compose => {
                if value["kind"] != "dataCompose" || value["composedHandle"].as_str().is_none() {
                    return invalid_response();
                }
                self.record_state(&value, "composedHandle");
                self.composed = value;
                self.export()
            }
            Phase::Export => {
                if value["kind"] != "dataExport"
                    || !value["exportHandle"].is_string()
                    || !value["objects"].is_array()
                {
                    return invalid_response();
                }
                let (Some(blob), Some(key), Some(content_type)) = (
                    value["blobHandle"].as_str(),
                    value["idempotencyKey"].as_str(),
                    value["contentType"].as_str(),
                ) else {
                    return invalid_response();
                };
                let revision = value["revision"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value["revision"].as_u64().map(|value| value.to_string()));
                let Some(revision) = revision else {
                    return invalid_response();
                };
                self.record_state(&value, "exportHandle");
                self.blob_handles.push(blob.to_owned());
                let Some(download) = &self.download else {
                    return invalid_response();
                };
                let mut headers = vec![
                    json!({"name":"Content-Type","value":content_type}),
                    json!({"name":"Idempotency-Key","value":key}),
                ];
                if download.status == 404 {
                    headers.push(
                        json!({"name":"X-NoriShell-Expected-Next-Revision","value":revision}),
                    );
                } else if let Some(etag) = &download.etag {
                    headers.push(json!({"name":"If-Match","value":etag}));
                } else {
                    return invalid_response();
                }
                let operation = NetworkFlow::start(&self.url, "put", headers, Some(blob));
                self.exported = value;
                self.phase = Phase::Upload;
                self.network = NetworkFlow::default();
                Transition::Call(operation)
            }
            Phase::Upload => match self.network.receive(&value) {
                Progress::Call(call) => Transition::Call(call),
                Progress::Failed(code) => Transition::Failed {
                    code,
                    http_status: None,
                },
                Progress::Complete(receipt) => {
                    if !(200..300).contains(&receipt.status) {
                        return http_failure(receipt.status);
                    }
                    self.receipt_handles.push(receipt.handle.clone());
                    self.upload = Some(receipt);
                    if self.composed.is_null() {
                        self.checkpoint()
                    } else {
                        self.phase = Phase::Apply;
                        Transition::Call(
                            json!({"kind":"dataApply","request":{"profileId":"primary",
                            "categories":sync_policy::CATEGORIES,"expectedLocalSnapshotHandle":self.local["snapshotHandle"],
                                    "composedHandle":self.composed["composedHandle"],
                                    "authoritativeReceiptHandle":self.upload.as_ref().map(|receipt| &receipt.handle),
                                    "exportHandle":self.exported["exportHandle"]}}),
                        )
                    }
                }
            },
            Phase::Apply => {
                if value["kind"] != "dataApply" || value["applyReceiptHandle"].as_str().is_none() {
                    return invalid_response();
                }
                self.record_state(&value, "applyReceiptHandle");
                self.applied = value;
                self.checkpoint()
            }
            Phase::Checkpoint => {
                if value["kind"] != "dataCheckpoint" {
                    return invalid_response();
                }
                Transition::Finished {
                    difference: "equal",
                    review: false,
                }
            }
        }
    }

    fn source_handle(&self) -> &Value {
        if self.composed.is_null() {
            &self.local["snapshotHandle"]
        } else {
            &self.composed["composedHandle"]
        }
    }

    fn record_state(&mut self, value: &Value, key: &str) {
        if let Some(handle) = value[key].as_str() {
            self.state_handles.push(handle.to_owned());
        }
    }

    fn export(&mut self) -> Transition {
        let Some(download) = &self.download else {
            return invalid_response();
        };
        self.phase = Phase::Export;
        Transition::Call(json!({"kind":"dataExport","request":{"profileId":"primary",
            "categories":sync_policy::CATEGORIES,"sourceHandle":self.source_handle(),"baseReceiptHandle":download.handle}}))
    }

    fn checkpoint(&mut self) -> Transition {
        let Some(upload) = &self.upload else {
            return invalid_response();
        };
        self.phase = Phase::Checkpoint;
        Transition::Call(
            json!({"kind":"dataCheckpoint","request":{"profileId":"primary",
                "categories":sync_policy::CATEGORIES,"sourceHandle":self.source_handle(),
                "authoritativeReceiptHandle":upload.handle,"applyReceiptHandle":self.applied["applyReceiptHandle"],
                "exportHandle":self.exported["exportHandle"],"baseReceiptHandle":self.download.as_ref().map(|receipt| &receipt.handle)}}),
        )
    }
}

pub fn items(value: &Value) -> Result<Vec<Item>, ()> {
    value["objects"]
        .as_array()
        .ok_or(())?
        .iter()
        .map(|object| {
            Ok(Item {
                id: format!(
                    "{}:{}",
                    object["kind"].as_str().ok_or(())?,
                    object["stableId"].as_str().ok_or(())?
                ),
                handle: object["objectHandle"].as_str().ok_or(())?.to_owned(),
                equality_tag: object["equalityTag"].as_str().ok_or(())?.to_owned(),
                updated_at: object["updateTimeUnixMs"].as_i64(),
                deleted: object["tombstone"].as_bool().ok_or(())?,
            })
        })
        .collect()
}

pub fn display_rows(value: &Value) -> Vec<Value> {
    value["objects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|object| {
            matches!(
                object["kind"].as_str(),
                Some("host" | "credential" | "desktopProfile")
            ) && object["tombstone"] != true
        })
        .map(|object| {
            let mut row = object["display"].clone();
            if !row.is_object() {
                row = json!({});
            }
            row["category"] = object["category"].clone();
            row
        })
        .collect()
}

pub fn same_content(local: &[Item], remote: &[Item]) -> bool {
    local.len() == remote.len()
        && local.iter().all(|left| {
            remote.iter().any(|right| {
                left.id == right.id
                    && left.equality_tag == right.equality_tag
                    && left.deleted == right.deleted
            })
        })
}

fn objects_empty(value: &Value) -> bool {
    if value["keyPending"] == true {
        return ["hostCount", "credentialCount", "desktopProfileCount"]
            .iter()
            .all(|key| value["localCounts"][key].as_u64() == Some(0));
    }
    value["objects"].as_array().is_some_and(Vec::is_empty)
}
pub fn invalid_response() -> Transition {
    Transition::Failed {
        code: "invalidResponse".into(),
        http_status: None,
    }
}
pub fn http_failure(status: u16) -> Transition {
    Transition::Failed {
        code: match status {
            401 => "accountNotConnected",
            403 => "accessDenied",
            409 | 412 => "stateConflict",
            429 => "quotaExceeded",
            500..=599 => "serviceUnavailable",
            _ => "remoteRequestRejected",
        }
        .into(),
        http_status: Some(status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(tag: &str, time: i64) -> Value {
        json!({"category":"hosts","kind":"host","stableId":"one","objectHandle":"same-identity",
            "equalityTag":tag,"updateTimeUnixMs":time,"tombstone":false,"dependency":false,
            "display":{"kind":"host","label":"One","address":"example.org","port":22}})
    }

    #[test]
    fn remote_authentication_precedes_keyed_local_reread_and_merge() {
        let mut flow = Flow::new(
            Intent::Sync,
            "https://example.org/exchange".into(),
            "newest",
            "newest",
            None,
        );
        assert!(
            matches!(flow.receive(json!({"kind":"dataSnapshot","snapshotHandle":"initial","keyPending":true,"objects":[]})), Transition::Call(call) if call["kind"]=="networkStart")
        );
        flow.phase = Phase::Inspect;
        assert!(
            matches!(flow.receive(json!({"kind":"dataInspect","inspectionHandle":"remote","objects":[object("remote",20)],"migrationRequired":false})), Transition::Call(call) if call["kind"]=="dataSnapshot")
        );
        let result = flow.receive(json!({"kind":"dataSnapshot","snapshotHandle":"keyed-local","keyPending":false,"objects":[object("local",10)]}));
        let Transition::Call(call) = result else {
            panic!("compose required")
        };
        assert_eq!(call["kind"], "dataCompose");
        assert_eq!(call["request"]["localSnapshotHandle"], "keyed-local");
        assert_eq!(
            call["request"]["decisions"],
            json!([{"objectHandle":"same-identity","source":"remote"}])
        );
    }

    #[test]
    fn pending_key_uses_protected_local_counts_for_remote_absence() {
        assert!(!objects_empty(
            &json!({"keyPending":true,"objects":[],"localCounts":{
            "hostCount":1,"credentialCount":0,"desktopProfileCount":0}})
        ));
        assert!(objects_empty(
            &json!({"keyPending":true,"objects":[],"localCounts":{
            "hostCount":0,"credentialCount":0,"desktopProfileCount":0}})
        ));
    }

    #[test]
    fn collected_snapshot_handle_is_released_when_flow_stops() {
        let mut flow = Flow::new(
            Intent::Refresh,
            "https://example.org/exchange".into(),
            "newest",
            "newest",
            None,
        );
        assert!(matches!(
            flow.receive_snapshot(
                json!({"kind":"dataSnapshot","snapshotHandle":"snapshot-1","objects":[]})
            ),
            Transition::Call(_)
        ));
        assert_eq!(
            flow.release_request()["request"]["stateHandles"],
            json!(["snapshot-1"])
        );
    }

    #[test]
    fn equal_sync_checkpoints_authenticated_get_before_success() {
        let mut flow = Flow::new(
            Intent::Sync,
            "https://example.org/exchange".into(),
            "newest",
            "newest",
            None,
        );
        flow.local = json!({"snapshotHandle":"local","objects":[object("same",10)]});
        flow.remote = json!({"inspectionHandle":"remote","objects":[object("same",10)],"migrationRequired":false});
        flow.download = Some(Receipt {
            handle: "get-receipt".into(),
            status: 200,
            blob: Some("get-blob".into()),
            etag: Some("\"v1\"".into()),
        });
        let Transition::Call(call) = flow.decide() else {
            panic!("equal data must establish a baseline")
        };
        assert_eq!(call["kind"], "dataCheckpoint");
        assert_eq!(call["request"]["expectedLocalSnapshotHandle"], "local");
        assert_eq!(call["request"]["remoteInspectionHandle"], "remote");
        assert_eq!(call["request"]["authoritativeReceiptHandle"], "get-receipt");
    }

    #[test]
    fn ambiguous_clocks_stop_before_export() {
        let mut flow = Flow::new(
            Intent::Sync,
            "https://example.org/exchange".into(),
            "newest",
            "newest",
            None,
        );
        flow.local = json!({"snapshotHandle":"local","objects":[object("local",10)]});
        flow.remote = json!({"inspectionHandle":"remote","objects":[object("remote",10)]});
        assert!(matches!(
            flow.decide(),
            Transition::Finished { review: true, .. }
        ));
        assert!(flow.exported.is_null());
        flow.choice = Some(Source::Remote);
        assert!(
            matches!(flow.decide(),Transition::Call(call) if call["request"]["decisions"][0]["source"]=="remote")
        );
    }

    #[test]
    fn upload_acknowledgement_requires_apply_and_checkpoint_before_success() {
        let mut flow = Flow::new(
            Intent::Sync,
            "https://example.org/exchange".into(),
            "newest",
            "newest",
            None,
        );
        flow.local = json!({"snapshotHandle":"local","objects":[object("local",10)]});
        flow.composed = json!({"composedHandle":"composed","objects":[object("remote",20)]});
        flow.exported = json!({"exportHandle":"exported"});
        flow.phase = Phase::Upload;
        assert!(matches!(
            flow.receive(json!({"kind":"networkStarted","handle":"network"})),
            Transition::Call(_)
        ));
        let receipt = json!({"kind":"resourceEvents","handle":"network","backpressured":false,"events":[{
            "kind":{"kind":"network","event":{"kind":"httpExchangeCompleted","receiptHandle":"uploaded","status":200,"etag":"\"v2\""}}
        }]});
        assert!(
            matches!(flow.receive(receipt),Transition::Call(call) if call["kind"]=="resourceClose")
        );
        assert!(
            matches!(flow.receive(json!({"kind":"closed","handle":"network"})),Transition::Call(call) if call["kind"]=="dataApply" && call["request"]["authoritativeReceiptHandle"]=="uploaded" && call["request"]["exportHandle"]=="exported")
        );
        assert!(
            matches!(flow.receive(json!({"kind":"dataApply","applyReceiptHandle":"applied"})),Transition::Call(call) if call["kind"]=="dataCheckpoint" && call["request"]["applyReceiptHandle"]=="applied")
        );
        assert!(matches!(
            flow.receive(json!({"kind":"dataCheckpoint","syncedAtUnixMs":30})),
            Transition::Finished {
                difference: "equal",
                review: false
            }
        ));
    }
}
