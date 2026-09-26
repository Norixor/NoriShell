//! Plugin-side HTTP sequencing. Network bytes and OAuth tokens stay inside Core.

use serde_json::{Value, json};

pub const MAX_EXCHANGE_BYTES: u32 = 96 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Receipt {
    pub handle: String,
    pub status: u16,
    pub etag: Option<String>,
    pub blob: Option<String>,
}

#[derive(Default)]
pub struct NetworkFlow {
    handle: Option<String>,
    waits: u8,
    pending_result: Option<Result<Receipt, String>>,
    was_started: bool,
}

pub enum Progress {
    Call(Value),
    Complete(Receipt),
    Failed(String),
}

impl NetworkFlow {
    pub fn started(&self) -> bool {
        self.was_started
    }
    pub fn start(url: &str, method: &str, headers: Vec<Value>, body: Option<&str>) -> Value {
        json!({"kind":"networkStart","endpoint":{"endpoint":url},"request":{
            "timeoutMs":30_000,"oauthProfileId":"primary","operation":{
                "kind":"httpExchange","method":method,"headers":headers,
                "profileId":"primary","bodyBlobHandle":body,"maxResponseBytes":MAX_EXCHANGE_BYTES
            }
        }})
    }

    pub fn receive(&mut self, result: &Value) -> Progress {
        if self.pending_result.is_some() {
            if result["kind"] != "closed" || result["handle"].as_str() != self.handle.as_deref() {
                return Progress::Failed("cleanupIncomplete".into());
            }
            self.handle = None;
            return match self.pending_result.take().expect("pending result") {
                Ok(receipt) => Progress::Complete(receipt),
                Err(code) => Progress::Failed(code),
            };
        }
        match result["kind"].as_str() {
            Some("networkStarted") if self.handle.is_none() => {
                let Some(handle) = result["handle"]
                    .as_str()
                    .filter(|handle| !handle.is_empty())
                else {
                    return Progress::Failed("invalidResponse".into());
                };
                self.handle = Some(handle.to_owned());
                self.was_started = true;
                self.wait()
            }
            Some("resourceEvents")
                if result["handle"].as_str() == self.handle.as_deref() && self.handle.is_some() =>
            {
                let Some(events) = result["events"].as_array() else {
                    return self.finish(Err("invalidResponse".into()));
                };
                if result["backpressured"] == true {
                    return self.finish(Err("outcomeUnknown".into()));
                }
                for event in events {
                    let event = &event["kind"];
                    if event["kind"] == "cancelled" {
                        return self.finish(Err("cancelled".into()));
                    }
                    if event["kind"] != "network" {
                        continue;
                    }
                    let event = &event["event"];
                    match event["kind"].as_str() {
                        Some("httpExchangeCompleted") => {
                            let Some(status) = event["status"]
                                .as_u64()
                                .and_then(|status| u16::try_from(status).ok())
                            else {
                                return self.finish(Err("invalidResponse".into()));
                            };
                            let Some(handle) = event["receiptHandle"]
                                .as_str()
                                .filter(|handle| !handle.is_empty())
                            else {
                                return self.finish(Err("invalidResponse".into()));
                            };
                            return self.finish(Ok(Receipt {
                                handle: handle.to_owned(),
                                status,
                                etag: event["etag"].as_str().map(str::to_owned),
                                blob: event["bodyBlobHandle"].as_str().map(str::to_owned),
                            }));
                        }
                        Some("error") => {
                            return self.finish(Err(event["code"]
                                .as_str()
                                .unwrap_or("networkUnavailable")
                                .to_owned()));
                        }
                        Some("closed") => return self.finish(Err("outcomeUnknown".into())),
                        _ => {}
                    }
                }
                self.wait()
            }
            _ if self.handle.is_some() => self.finish(Err("invalidResponse".into())),
            _ => Progress::Failed("invalidResponse".into()),
        }
    }

    fn finish(&mut self, result: Result<Receipt, String>) -> Progress {
        self.pending_result = Some(result);
        Progress::Call(json!({"kind":"resourceClose","handle":self.handle}))
    }

    fn wait(&mut self) -> Progress {
        self.waits += 1;
        if self.waits > 2 {
            return self.finish(Err("outcomeUnknown".into()));
        }
        Progress::Call(json!({"kind":"resourceEvents","handle":self.handle,
            "limit":32,"waitMs":30_000}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_opened_but_waits_for_completion_receipt() {
        let mut network = NetworkFlow::default();
        assert!(matches!(
            network.receive(&json!({"kind":"networkStarted","handle":"network"})),
            Progress::Call(_)
        ));
        let result = network.receive(&json!({"kind":"resourceEvents","handle":"network","backpressured":false,
            "events":[{"kind":{"kind":"network","event":{"kind":"opened"}}},
                {"kind":{"kind":"network","event":{"kind":"httpExchangeCompleted","receiptHandle":"receipt",
                    "status":200,"etag":"\"v1\"","bodyBlobHandle":"blob"}}}]}));
        assert!(matches!(result, Progress::Call(call) if call["kind"]=="resourceClose"));
        assert!(
            matches!(network.receive(&json!({"kind":"closed","handle":"network"})), Progress::Complete(receipt) if receipt.handle=="receipt" && receipt.blob.as_deref()==Some("blob"))
        );
    }
}
