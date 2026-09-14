//! Emits a plan for isolated command-semantic QA. No network or execution.
use base64::Engine;
use norishell_plugin_platform::operations::{RemoteOperation, plan};
use std::io::Read;

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .take(128 * 1024)
        .read_to_string(&mut input)
        .expect("read operation JSON");
    let operation: RemoteOperation = serde_json::from_str(&input).expect("strict operation JSON");
    let plan = plan(&operation).expect("validated operation");
    println!(
        "{}",
        serde_json::json!({"command":plan.command,"stdinBase64":plan.stdin.map(|bytes|base64::engine::general_purpose::STANDARD.encode(bytes)),"timeoutSeconds":plan.timeout.as_secs(),"maxOutputBytes":plan.max_output_bytes,"class":format!("{:?}",plan.class)})
    );
}
