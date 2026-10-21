//! Decode a Bink file: bikdump <file.bik> <out dir> [frame,frame,...]
//! Saves those frames as PNG, the first audio track as WAV, and the
//! decode speed.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&a[0])?;
    let out = std::path::PathBuf::from(a.get(1).cloned().unwrap_or("bikdump".into()));
    std::fs::create_dir_all(&out)?;
    let want: Vec<usize> = a.get(2).map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or(vec![0, 30, 150, 600]);
    let mut b = bink::Bink::open(data)?;
    println!("{}x{} {:.3} fps, {} frames, rev {}, tracks {:?}", b.width, b.height, b.fps, b.frame_count(), b.revision(), b.tracks.iter().map(|t| (t.sample_rate, t.channels)).collect::<Vec<_>>());
    let last = *want.iter().max().unwrap_or(&0);
    let mut rgba = Vec::new();
    let mut samples: Vec<f32> = Vec::new();
    let t0 = std::time::Instant::now();
    while let Some(f) = b.next_frame()? {
        if let Some(s) = f.audio.first() {
            samples.extend_from_slice(s);
        }
        if want.contains(&f.index) {
            b.rgba(&mut rgba);
            image::save_buffer(out.join(format!("frame{:04}.png", f.index)), &rgba, b.width as u32, b.height as u32, image::ColorType::Rgba8)?;
        }
        if f.index >= last {
            break;
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    println!("decoded {} frames in {secs:.2}s ({:.0} fps)", last + 1, (last + 1) as f64 / secs);
    if let Some(t) = b.tracks.first() {
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        println!("audio: {} samples, peak {peak:.3}", samples.len());
        let mut wav = Vec::new();
        let ch = t.channels as u16;
        let n = samples.len() as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + n * 2).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&ch.to_le_bytes());
        wav.extend_from_slice(&t.sample_rate.to_le_bytes());
        wav.extend_from_slice(&(t.sample_rate * ch as u32 * 2).to_le_bytes());
        wav.extend_from_slice(&(ch * 2).to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(n * 2).to_le_bytes());
        for s in &samples {
            wav.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(out.join("audio.wav"), wav)?;
    }
    Ok(())
}
