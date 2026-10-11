// Prints where static models whose name contains the given text stand.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let w = z.gfx_world().unwrap();
    for s in w.static_models.iter().filter(|s| s.model.and_then(|m| z.xmodel(m)).is_some_and(|m| m.name.contains(&a[1]))).take(8) {
        println!("{:?} {:?} axis {:?} scale {}", z.xmodel(s.model.unwrap()).unwrap().name, s.origin, s.axis, s.scale);
    }
    Ok(())
}
