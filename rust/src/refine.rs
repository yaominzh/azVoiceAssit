use serde_json::{json, Value};
use std::time::Duration;

pub fn build_omlx_body(messages: Vec<Value>) -> Value {
    json!({
        "model": crate::config::OMLX_MODEL,
        "messages": messages,
        "temperature": 0.3
    })
}

/// Call oMLX to refine text. Returns the stripped assistant reply or an error string.
pub fn refine(client: &reqwest::blocking::Client, messages: Vec<Value>) -> Result<String, String> {
    let resp = client
        .post(crate::config::OMLX_URL)
        .bearer_auth(crate::config::OMLX_API_KEY)
        .json(&build_omlx_body(messages))
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|e| format!("oMLX send: {e}"))?;
    let v: Value = resp.json().map_err(|e| format!("oMLX json: {e}"))?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "oMLX: missing choices[0].message.content".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_has_model_messages_temperature() {
        let msgs = vec![json!({"role": "user", "content": "hi"})];
        let body = build_omlx_body(msgs.clone());
        assert_eq!(body["model"], crate::config::OMLX_MODEL);
        assert_eq!(body["messages"], json!(msgs));
        assert_eq!(body["temperature"], 0.3);
    }
}
