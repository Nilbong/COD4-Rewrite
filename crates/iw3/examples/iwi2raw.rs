//! Decode an iwi's top mip to raw RGBA: iwi2raw <image name> <out.raw>
use anyhow::Result;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().expect("image name");
    let out = args.next().expect("output path");
    let install = iw3::Install::locate()?;
    let vfs = iw3::iwd::Vfs::mount(&install.iwd_paths()?)?;
    let bytes = vfs.read(&format!("images/{name}.iwi"))?.expect("not found");
    let iwi = iw3::iwi::Iwi::parse(&bytes)?;
    println!("{}x{} {:?} levels {}", iwi.width, iwi.height, iwi.format, iwi.levels.len());
    std::fs::write(out, iwi.to_rgba8(0).unwrap_or_else(|| iwi.levels[0].clone()))?;
    Ok(())
}
