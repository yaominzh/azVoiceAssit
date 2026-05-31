fn main() {
    let client = reqwest::blocking::Client::new();
    let messages = vec![
        serde_json::json!({"role": "system", "content": "You are a refinement assistant. Repeat the utterance back cleaned up. Reply with ONLY the refined sentence."}),
        serde_json::json!({"role": "user", "content": "um so like the meetin is uh tomorrow"}),
    ];
    match azva::refine::refine(&client, messages) {
        Ok(text) => println!("Refined: {text}"),
        Err(e) => eprintln!("Error: {e}"),
    }
}
