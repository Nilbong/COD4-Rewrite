//! Everything with a model near a point in a converted MW2 map (entities,
//! static models, dynamic entities): `near <map> x y z [r]`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let p: Vec<f32> = a[1..4].iter().map(|v| v.parse().unwrap()).collect();
    let r: f32 = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(150.0);
    let close = |o: [f32; 3]| (0..3).all(|k| (o[k] - p[k]).abs() < r);
    for e in iw3::ents::parse(&z.map_ents().unwrap().entity_string) {
        if let (Some(o), Some(m)) = (e.origin(), e.get("model")) {
            if close(o) { println!("ent {} {m} {:?} {:?}", e.classname(), o, e.get("targetname")); }
        }
    }
    let w = z.gfx_world().unwrap();
    for s in &w.static_models { if close(s.origin) { println!("smodel {:?} {:?} cull {} flags {:#x} scale {} axis {:?}", s.model.and_then(|m| z.xmodel(m)).map(|m| &m.name), s.origin, s.cull_dist, s.flags, s.scale, s.axis); } }
    for d in &z.clip_map().unwrap().dyn_ents { if close(d.origin) { println!("dynent kind {} {:?} brush {} {:?}", d.kind, d.model.and_then(|m| z.xmodel(m)).map(|m| &m.name), d.brush_model, d.origin); } }
    Ok(())
}
