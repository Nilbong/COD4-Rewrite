//! Average colour of Black Ops images: iwistat <image>...
fn main() -> anyhow::Result<()> {
    let install = t5::Install::locate()?;
    let vfs = install.vfs()?;
    for name in std::env::args().skip(1) {
        let Some(bytes) = vfs.read(&format!("images/{name}.iwi"))? else { println!("{name}: not found"); continue };
        let iwi = iw3::iwi::Iwi::parse(&bytes)?;
        let Some(rgba) = iwi.to_rgba8(0) else {
            // Compressed: write the top mip's blocks for an outside decoder.
            let out = std::env::temp_dir().join(format!("{name}.raw"));
            std::fs::write(&out, &iwi.levels[0])?;
            println!("{name}: {}x{} {:?} raw -> {}", iwi.width, iwi.height, iwi.format, out.display());
            continue;
        };
        let n = (rgba.len() / 4) as f64;
        let mut sum = [0f64; 4];
        for px in rgba.chunks_exact(4) {
            for c in 0..4 {
                sum[c] += px[c] as f64 / 255.0;
            }
        }
        println!("{name}: {}x{} {:?} mean rgba {:.2} {:.2} {:.2} {:.2}", iwi.width, iwi.height, iwi.format, sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n);
    }
    Ok(())
}
