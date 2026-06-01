/// AEC Phase 1: shadow mode — observes TTS reference + mic, logs cancellation.
/// Does NOT feed cancelled output to VAD yet. Half-duplex speaking gate unchanged.
pub struct EchoCancel {
    // populated in Task 3
}

/// Convert f32 [-1.0, 1.0] → i16 with saturation clamping.
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

/// Convert i16 → f32 [-1.0, 1.0].
pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&x| x as f32 / 32767.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_positive_clamp() {
        assert_eq!(f32_to_i16(&[1.0]), vec![32767]);
        assert_eq!(f32_to_i16(&[2.0]), vec![32767]); // clamped
    }

    #[test]
    fn f32_to_i16_negative_clamp() {
        assert_eq!(f32_to_i16(&[-1.0]), vec![-32767]);
        assert_eq!(f32_to_i16(&[-2.0]), vec![-32767]); // clamped
    }

    #[test]
    fn f32_to_i16_zero() {
        assert_eq!(f32_to_i16(&[0.0]), vec![0]);
    }

    #[test]
    fn i16_to_f32_roundtrip_within_epsilon() {
        let original = vec![0.5f32, -0.5, 0.0, 0.999];
        let converted = f32_to_i16(&original);
        let back = i16_to_f32(&converted);
        for (a, b) in original.iter().zip(back.iter()) {
            assert!((a - b).abs() < 0.001, "roundtrip error: {a} vs {b}");
        }
    }

    #[test]
    fn f32_to_i16_batch() {
        let input = vec![0.0f32; 512];
        assert_eq!(f32_to_i16(&input).len(), 512);
    }
}
