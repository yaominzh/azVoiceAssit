use std::collections::VecDeque;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct History {
    turns: VecDeque<Value>,
    cap: usize,
}

impl History {
    pub fn new(cap: usize) -> Self {
        Self { turns: VecDeque::new(), cap }
    }

    pub fn len(&self) -> usize { self.turns.len() }

    pub fn is_empty(&self) -> bool { self.turns.is_empty() }

    fn push(&mut self, v: Value) {
        if self.turns.len() == self.cap && self.cap > 0 {
            self.turns.pop_front();
        }
        if self.cap > 0 {
            self.turns.push_back(v);
        }
    }

    /// Append the user turn, then return [system] + window for the oMLX request.
    pub fn record_user_and_build(&mut self, text: &str, system: &str) -> Vec<Value> {
        self.push(json!({"role": "user", "content": text}));
        let mut msgs = vec![json!({"role": "system", "content": system})];
        msgs.extend(self.turns.iter().cloned());
        msgs
    }

    pub fn record_assistant(&mut self, text: &str) {
        self.push(json!({"role": "assistant", "content": text}));
    }

    pub fn clear(&mut self) { self.turns.clear(); }

    #[cfg(test)]
    pub fn iter_contents(&self) -> Vec<String> {
        self.turns
            .iter()
            .map(|v| v["content"].as_str().unwrap_or("").to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_user_then_build_messages_includes_system_and_window() {
        let mut h = History::new(40);
        let msgs = h.record_user_and_build("um the meeting tomorrow", "SYS");
        assert_eq!(msgs[0], serde_json::json!({"role":"system","content":"SYS"}));
        assert_eq!(
            *msgs.last().unwrap(),
            serde_json::json!({"role":"user","content":"um the meeting tomorrow"})
        );
        h.record_assistant("The meeting is tomorrow.");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::new(4); // 2 turns max
        for (u, a) in [("one", "r1"), ("two", "r2"), ("three", "r3")] {
            h.record_user_and_build(u, "SYS");
            h.record_assistant(a);
        }
        assert_eq!(h.len(), 4);
        let contents: Vec<String> = h.iter_contents();
        assert!(!contents.contains(&"one".to_string())); // oldest dropped
        assert!(contents.contains(&"three".to_string()));
    }
}
