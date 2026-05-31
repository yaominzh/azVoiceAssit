use ndarray::{Array1, Array2, Array3};
use ort::{session::Session, value::Tensor};

fn load_speech() -> (Vec<f32>, u32) {
    let bytes = std::fs::read("/tmp/mic_test.wav").expect("need /tmp/mic_test.wav");
    let src_rate = u32::from_le_bytes([bytes[24],bytes[25],bytes[26],bytes[27]]);
    let samples: Vec<f32> = bytes[44..].chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0],b[1]]) as f32 / 32768.0).collect();
    let ratio = src_rate as f64 / 16000.0;
    let out_len = (samples.len() as f64 / ratio) as usize;
    let ds: Vec<f32> = (0..out_len).map(|i|{
        let pos=i as f64*ratio; let lo=pos.floor() as usize; let hi=(lo+1).min(samples.len()-1);
        let t=(pos-pos.floor()) as f32; samples[lo]*(1.0-t)+samples[hi]*t
    }).collect();
    (ds, src_rate)
}

fn main() {
    let mut session = Session::builder().unwrap()
        .commit_from_file("models/silero_vad.onnx").unwrap();
    let (ds, _) = load_speech();
    // find loudest 512-frame
    let loud = ds.chunks_exact(512)
        .max_by(|a,b| a.iter().map(|x|x.abs()).fold(0f32,f32::max)
            .partial_cmp(&b.iter().map(|x|x.abs()).fold(0f32,f32::max)).unwrap())
        .unwrap().to_vec();
    println!("loudest frame peak={:.3}", loud.iter().cloned().fold(0f32,|a,b|a.max(b.abs())));

    let state = Array3::<f32>::zeros([2,1,128]);

    // Try sr as [1,1], [1], and scalar []
    for label in ["[[sr]] shape[1,1]", "[sr] shape[1]", "scalar []"] {
        let audio = Tensor::<f32>::from_array(Array2::from_shape_vec([1,512], loud.clone()).unwrap()).unwrap();
        let st = Tensor::<f32>::from_array(state.clone()).unwrap();
        let sr = match label {
            "[[sr]] shape[1,1]" => Tensor::<i64>::from_array(ndarray::array![[16000i64]]).unwrap(),
            "[sr] shape[1]"     => Tensor::<i64>::from_array(Array1::from_vec(vec![16000i64])).unwrap(),
            _ => Tensor::<i64>::from_array(((vec![] as Vec<i64>, vec![16000i64]))).unwrap(),
        };
        match session.run(ort::inputs!["input"=>audio,"state"=>st,"sr"=>sr]) {
            Ok(outputs) => {
                let (_, p) = outputs["output"].try_extract_tensor::<f32>().unwrap();
                println!("sr={:<18} -> prob={:.4}", label, p[0]);
            }
            Err(e) => println!("sr={:<18} -> ERROR {e}", label),
        }
    }
}
