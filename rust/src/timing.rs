#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnTiming {
    pub endpoint_ms: u32,
    pub stt_ms: u32,
    pub refine_ms: u32,
    pub reply_start_ms: u32,
}

impl TurnTiming {
    pub fn format(&self) -> String {
        format!(
            "endpoint ~{}ms \u{00B7} stt {}ms \u{00B7} refine {}ms \u{00B7} reply-start +{}ms",
            self.endpoint_ms, self.stt_ms, self.refine_ms, self.reply_start_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_line() {
        let t = TurnTiming {
            endpoint_ms: 700,
            stt_ms: 240,
            refine_ms: 180,
            reply_start_ms: 430,
        };
        assert_eq!(
            t.format(),
            "endpoint ~700ms \u{00B7} stt 240ms \u{00B7} refine 180ms \u{00B7} reply-start +430ms"
        );
    }
}
