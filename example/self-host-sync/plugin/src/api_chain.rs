//! Correlation for one provider-owned broker sequence.

use norishell_plugin_sdk::{PluginError, PluginRuntimeOutput, output};
use serde_json::{Value, json};

#[derive(Default)]
pub struct ApiChain {
    expected_call_id: Option<String>,
    sequence: u32,
}

pub enum Reply {
    Completed(Value),
    Failed(String),
}

impl ApiChain {
    pub fn request(
        &mut self,
        request_id: &str,
        operation: Value,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        if self.expected_call_id.is_some() {
            return Err(PluginError::InvalidRequest);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(PluginError::InvalidRequest)?;
        let call_id = format!("self-host.{}", self.sequence);
        self.expected_call_id = Some(call_id.clone());
        Ok(vec![output(
            request_id,
            "api.request",
            &json!({"callId":call_id,"operation":operation}),
        )?])
    }

    pub fn reply(&mut self, body: &Value) -> Result<Reply, PluginError> {
        let result = &body["result"];
        let reply = &result["reply"];
        if result["kind"] != "api"
            || self.expected_call_id.as_deref().is_none()
            || reply["callId"].as_str() != self.expected_call_id.as_deref()
        {
            return Err(PluginError::InvalidRequest);
        }
        self.expected_call_id = None;
        match reply["outcome"]["kind"].as_str() {
            Some("completed") => Ok(Reply::Completed(reply["outcome"]["value"].clone())),
            Some("failed") => Ok(Reply::Failed(
                reply["outcome"]["code"]
                    .as_str()
                    .ok_or(PluginError::InvalidRequest)?
                    .to_owned(),
            )),
            _ => Err(PluginError::InvalidRequest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unrelated_or_duplicate_callback() {
        let mut chain = ApiChain::default();
        chain
            .request("host-one", json!({"kind":"describe"}))
            .unwrap();
        let mut callback = json!({"result":{"kind":"api","reply":{"callId":"foreign",
            "outcome":{"kind":"completed","value":{"kind":"description"}}}}});
        assert!(chain.reply(&callback).is_err());
        callback["result"]["reply"]["callId"] = json!("self-host.1");
        assert!(matches!(chain.reply(&callback), Ok(Reply::Completed(_))));
        assert!(chain.reply(&callback).is_err());
    }
}
