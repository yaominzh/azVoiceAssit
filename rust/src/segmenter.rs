use std::collections::VecDeque;

pub struct Segmenter {
    preroll: VecDeque<Vec<f32>>,
    preroll_cap: usize,
    buffer: Vec<Vec<f32>>,
    collecting: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VadEvent { Start, End }

impl Segmenter {
    pub fn new(preroll_frames: usize) -> Self {
        Self {
            preroll: VecDeque::new(),
            preroll_cap: preroll_frames,
            buffer: Vec::new(),
            collecting: false,
        }
    }

    /// Returns a flattened utterance (pre-roll + speech) on End, else None.
    pub fn push(&mut self, frame: Vec<f32>, event: Option<VadEvent>) -> Option<Vec<f32>> {
        match event {
            Some(VadEvent::Start) => {
                self.collecting = true;
                self.buffer = self.preroll.drain(..).collect();
                self.buffer.push(frame);
                None
            }
            // Note: if End arrives without a prior Start, collecting==false and this arm
            // doesn't fire — the frame is silently treated as silence and rotates into the
            // pre-roll ring. That is harmless; the VAD should never emit End without Start.
            _ if self.collecting => {
                self.buffer.push(frame);
                if event == Some(VadEvent::End) {
                    self.collecting = false;
                    let utt: Vec<f32> = self.buffer.drain(..).flatten().collect();
                    Some(utt)
                } else {
                    None
                }
            }
            _ => {
                if self.preroll.len() == self.preroll_cap && self.preroll_cap > 0 {
                    self.preroll.pop_front();
                }
                if self.preroll_cap > 0 {
                    self.preroll.push_back(frame);
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(v: f32) -> Vec<f32> { vec![v; 4] }

    #[test]
    fn emits_nothing_before_start() {
        let mut s = Segmenter::new(2);
        assert!(s.push(frame(0.0), None).is_none());
        assert!(s.push(frame(0.0), None).is_none());
    }

    #[test]
    fn prepends_preroll_on_start() {
        let mut s = Segmenter::new(2);
        s.push(frame(1.0), None);
        s.push(frame(1.0), None);
        assert!(s.push(frame(2.0), Some(VadEvent::Start)).is_none());
        let utt = s.push(frame(2.0), Some(VadEvent::End)).unwrap();
        // 2 preroll + start + end = 4 frames of 4 samples = 16
        assert_eq!(utt.len(), 16);
        assert_eq!(utt[0], 1.0);
        assert_eq!(*utt.last().unwrap(), 2.0);
    }

    #[test]
    fn resets_between_utterances() {
        let mut s = Segmenter::new(1);
        s.push(frame(1.0), Some(VadEvent::Start));
        s.push(frame(1.0), Some(VadEvent::End));
        s.push(frame(3.0), Some(VadEvent::Start));
        let utt = s.push(frame(3.0), Some(VadEvent::End)).unwrap();
        assert!(utt.iter().all(|&x| x == 3.0));
    }
}
