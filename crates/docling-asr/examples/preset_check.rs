//! Quick preset comparison: default vs a named preset on one audio file.
fn main() {
    let path = std::env::args().nth(1).expect("audio path");
    let preset = std::env::args().nth(2);
    let bytes = std::fs::read(&path).unwrap();
    let doc =
        docling_asr::convert_audio_with_model(&bytes, &path, preset.as_deref()).expect("converts");
    println!("{}", doc.export_to_markdown());
}
