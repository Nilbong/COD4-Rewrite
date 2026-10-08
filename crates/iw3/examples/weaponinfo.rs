//! Print a weapon definition: weaponinfo [name]
use anyhow::Result;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> Result<()> {
    let name = std::env::args().nth(1).unwrap_or("ak47_mp".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path("common_mp"))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let w = zone.assets.iter().find_map(|a| match a { Asset::Generic(g) if g.ty == AssetType::Weapon && g.name == name => Some(&g.root), _ => None }).expect("weapon");
    let model = |id: Option<usize>| id.map(|i| zone.get(i).name().to_owned());
    println!("gunXModel {:?}", w.assets("gunXModel").into_iter().map(model).collect::<Vec<_>>());
    println!("handXModel {:?}", model(w.asset("handXModel")));
    println!("worldModel {:?}", w.assets("worldModel").into_iter().map(model).take(2).collect::<Vec<_>>());
    println!("szXAnims {:?}", w.strings("szXAnims"));
    for f in ["damage","playerDamage","iFireTime","iClipSize","iMaxAmmo","iStartAmmo","iReloadTime","iReloadEmptyTime","iReloadAddTime","iRaiseTime","iDropTime","iAdsTransInTime","iAdsTransOutTime","sprintInTime","sprintLoopTime","sprintOutTime","iFireDelay","weapClass","fireType","penetrateType"] {
        print!("{f}={} ", w.int(f));
    }
    println!();
    for f in ["moveSpeedScale","adsMoveSpeedScale","fAdsZoomFov","fHipSpreadStandMin","fHipSpreadDuckedMin","fHipSpreadProneMin","hipSpreadStandMax","hipSpreadDuckedMax","hipSpreadProneMax","fHipSpreadDecayRate","fHipSpreadFireAdd","fHipSpreadMoveAdd","fAdsSpread","fHipViewKickPitchMin","fHipViewKickPitchMax","fHipViewKickYawMin","fHipViewKickYawMax","fHipViewKickCenterSpeed","fAdsViewKickPitchMin","fAdsViewKickPitchMax","fAdsViewKickYawMin","fAdsViewKickYawMax","fAdsViewKickCenterSpeed","fHipGunKickPitchMin","fHipGunKickPitchMax","fAdsGunKickPitchMin","fAdsGunKickPitchMax","fAdsBobFactor","fAdsViewBobMult","vStandRot[0]","vStandRot[1]","vStandRot[2]","vDuckedRot[0]","vDuckedRot[1]","vDuckedRot[2]","vProneRot[0]","vProneRot[1]","vProneRot[2]","fHipIdleAmount","hipIdleSpeed","fAdsIdleAmount","adsIdleSpeed","fIdleCrouchFactor","fIdleProneFactor","fStandRotMinSpeed","fDuckedRotMinSpeed","fProneRotMinSpeed","fPosRotRate","fPosProneRotRate","swayMaxAngle","swayLerpSpeed","swayPitchScale","swayYawScale","swayHorizScale","swayVertScale","adsSwayMaxAngle","adsSwayLerpSpeed","adsSwayPitchScale","adsSwayYawScale","adsSwayHorizScale","adsSwayVertScale","maxDamageRange","minDamageRange","fightDist","maxDist","fAdsAimPitch"] {
        print!("{f}={} ", w.float(f));
    }
    println!();
    print!("minDamage={} locationDamageMultipliers=", w.int("minDamage"));
    for i in 0..19 {
        print!("{} ", w.float(&format!("locationDamageMultipliers[{i}]")));
    }
    println!();
    Ok(())
}
