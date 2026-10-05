//! How often each eFlags bit is set on players in a demo: demoflags <file.dm_1>
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: demoflags <file.dm_1>");
    let demo = iw3::demo::read(&std::fs::read(path)?)?;
    let mut bits = [0usize; 32];
    let mut n = 0;
    let mut legs: std::collections::BTreeMap<u32, usize> = Default::default();
    for s in &demo.snapshots {
        for e in s.entities.iter().filter(|e| e.e_type() == 1) {
            n += 1;
            let f = e.int("lerp.eFlags");
            for (b, c) in bits.iter_mut().enumerate() {
                *c += (f >> b & 1) as usize;
            }
            *legs.entry(e.int("legsAnim") & 0x3ff).or_default() += 1;
        }
    }
    for (b, c) in bits.iter().enumerate().filter(|(_, c)| **c > 0) {
        println!("bit {b:2} (0x{:06x}): {:.1}%", 1u32 << b, 100.0 * *c as f32 / n as f32);
    }
    let mut legs: Vec<_> = legs.into_iter().collect();
    legs.sort_by_key(|l| std::cmp::Reverse(l.1));
    println!("common legsAnim: {:?}", &legs[..legs.len().min(12)]);
    Ok(())
}
