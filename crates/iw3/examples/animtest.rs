//! Decode every xanim in a zone and report failures; print a sample track.
use anyhow::Result;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("common_mp".into());
    let show = args.next().unwrap_or("pb_stand_alert".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let (mut ok, mut bad) = (0, 0);
    for a in &zone.assets {
        let Asset::Generic(g) = a else { continue };
        if g.ty != AssetType::XAnimParts { continue }
        match iw3::xanim::XAnim::from_node(&g.name, &g.root, &zone.script_strings) {
            Ok(x) => {
                ok += 1;
                if x.name == show {
                    println!("{} frames {} fps {} loop {} dur {:.2}s", x.name, x.num_frames, x.framerate, x.looping, x.duration());
                    for b in x.bones.iter().take(12) {
                        println!("  {:<20} rot0 {:?} trans0 {:?}", b.name, b.rot.sample(0.0), b.trans.sample(0.0));
                    }
                }
            }
            Err(e) => {
                bad += 1;
                if bad < 6 { println!("FAIL {e}"); }
            }
        }
    }
    println!("decoded {ok}, failed {bad}");
    Ok(())
}
