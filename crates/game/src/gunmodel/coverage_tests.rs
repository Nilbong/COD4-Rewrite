//! Asset-backed mastery coverage checks. Load one weapon zone for all guns
//! and attachment variants rather than one install/zone per test case.

use super::*;
use bevy::camera::primitives::MeshAabb;
use bevy::ecs::world::CommandQueue;
use bevy::mesh::VertexAttributeValues;
use iw3::menu::UiData;
use iw3::zone::{ParseOptions, Zone};

#[test]
#[ignore = "requires a local CoD4 asset installation"]
fn every_cod4_gun_has_mastery_surfaces_and_skinned_raised_diamonds() -> anyhow::Result<()> {
    let install = iw3::Install::locate()?;
    let vfs = Arc::new(iw3::iwd::Vfs::mount(&install.iwd_paths()?)?);
    let ui = Zone::parse(&iw3::fastfile::load(&install.zone_path("code_post_gfx_mp"))?, ParseOptions::default())?;
    let tables = UiData::from_zone(&ui).tables;
    let table = tables.iter().find(|t| t.name.eq_ignore_ascii_case("mp/statstable.csv")).expect("native weapon table");
    let guns: Vec<(String, Vec<String>)> = (0..table.rows)
        .filter_map(|r| {
            let key = table.get(r, 4)?.trim();
            let family = table.get(r, 2)?;
            let firearm = matches!(
                family,
                "weapon_assault" | "weapon_smg" | "weapon_lmg" | "weapon_shotgun" | "weapon_sniper" | "weapon_pistol"
            ) || (family == "weapon_projectile" && key == "rpg");
            (firearm && !key.is_empty()).then(|| {
                (key.to_owned(), table.get(r, 8).unwrap_or("").split_whitespace().map(str::to_owned).collect())
            })
        })
        .collect();
    assert_eq!(guns.len(), 28, "27 native firearm menu entries and the RPG");
    let common = Zone::parse(&iw3::fastfile::load(&install.zone_path("common_mp"))?, ParseOptions::default())?;
    assert!(common.stats.stopped_at.is_none(), "complete native weapon asset parse required");
    let mut content = Content::new(vec![common], vfs);
    let mut meshes = Assets::<Mesh>::default();
    let mut materials = Assets::<StandardMaterial>::default();
    let mut images = Assets::<Image>::default();
    let mut bindposes = Assets::<SkinnedMeshInverseBindposes>::default();
    let mut camo_materials = Assets::<CamoMaterial>::default();
    let mut checked_diamonds = HashSet::new();
    let mut checked_models = 0;
    let mut raised_surfaces = 0;
    for (weapon, attachments) in &guns {
        for (camo, finish) in [
            (6, platinum::Finish::Gold),
            (platinum::CAMO, platinum::Finish::Platinum),
            (platinum::DIAMOND, platinum::Finish::Diamond),
        ] {
            assert_eq!(platinum::Finish::selected(weapon, camo), Some(finish), "{weapon}");
            let model_camo = finish.model_camo(weapon);
            let mut defs = vec![format!("{weapon}_mp")];
            defs.extend(attachments.iter().map(|a| format!("{weapon}_{a}_mp")));
            for def in defs {
                for field in [GUN_MODEL, WORLD_MODEL] {
                    let (name, _) = gun_model(&content, &def, model_camo, field)
                        .unwrap_or_else(|| panic!("{weapon} {finish:?} {def} {field}: missing model"));
                    let original_gold = finish == platinum::Finish::Gold && platinum::Finish::native_gold(weapon);
                    if original_gold {
                        assert!(
                            name.contains("gold"),
                            "native gold geometry/material variant must remain: {def} {field} => {name}"
                        );
                    }
                    let prepared = content
                        .model(&name, &mut meshes, &mut materials, &mut images, &mut bindposes)
                        .unwrap_or_else(|| panic!("{name}: native model must prepare"));
                    assert!(!prepared.surfaces.is_empty(), "{name}: visible source geometry required");
                    let mut coated = platinum::surfaces(&mut content, &name, &mut materials, &mut images);
                    let (zi, id) = content.find(&name).unwrap();
                    let xm = content.zones[zi].xmodel(id).unwrap();
                    let lod = xm.lods.first().unwrap();
                    let entries: Vec<_> = (lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize)
                        .filter_map(|s| {
                            let mat = xm.materials.get(s).copied().flatten()?;
                            let name = content.zones[zi].material(mat)?.name.clone();
                            (!xm.surfs[s].verts.is_empty() && !xm.surfs[s].tris.is_empty()).then_some((s, mat, name))
                        })
                        .collect();
                    if original_gold {
                        let ids: Vec<_> =
                            entries.iter().filter(|(_, _, n)| n.ends_with("_gold")).map(|(_, m, _)| *m).collect();
                        for mid in ids {
                            coated.insert(
                                content
                                    .material(zi, mid, &mut materials, &mut images)
                                    .expect("native gold material")
                                    .handle
                                    .id(),
                            );
                        }
                    }
                    assert!(!coated.is_empty(), "{weapon} {finish:?} {def} {field}: at least one coated body surface");
                    assert!(
                        prepared.surfaces.iter().any(|(_, m)| coated.contains(&m.id())),
                        "{name}: coverage must match spawned surfaces"
                    );
                    for (surface, mid, material_name) in entries {
                        let mat = content.material(zi, mid, &mut materials, &mut images).unwrap();
                        if !coated.contains(&mat.handle.id()) {
                            continue;
                        }
                        let name = material_name.trim_start_matches(',');
                        assert!(
                            ![
                                "misc_nocamo",
                                "scope",
                                "lens",
                                "reticle",
                                "reflex",
                                "acog",
                                "ironsight",
                                "sights",
                                "bullet",
                                "suppressor",
                                "foregrip",
                                "peq",
                                "trinium"
                            ]
                            .iter()
                            .any(|excluded| name.contains(excluded)),
                            "{name}: optics/misc/ammo must remain original"
                        );
                        if finish != platinum::Finish::Diamond || !checked_diamonds.insert((zi, id, surface)) {
                            continue;
                        }
                        let xm = content.zones[zi].xmodel(id).unwrap();
                        let source = crate::content::surface_mesh(xm, surface, true).expect("coated native mesh");
                        let result = diamond::studded(&source).unwrap_or_else(|| {
                            panic!("{weapon} {def} {field} {name} surface {surface}: raised geometry required")
                        });
                        check_raised_mesh(
                            &source,
                            &result,
                            xm.num_bones as usize,
                            &format!("{weapon} {def} {field} {name} {surface}"),
                        );
                        raised_surfaces += 1;
                    }
                    checked_models += 1;
                }
            }
        }
    }
    // Exercise composed attachment spawning as well as native single-variant
    // model resolution: this path grafts suppressors and hides the iron tags.
    let mut camos = CamoCache::default();
    let mut world = World::default();
    for (weapon, attachments) in &guns {
        let combo = if attachments.iter().any(|a| a == "reflex") && attachments.iter().any(|a| a == "silencer") {
            "reflex+silencer"
        } else if attachments.iter().any(|a| a == "grip") && attachments.iter().any(|a| a == "reflex") {
            "reflex+grip"
        } else if attachments.iter().any(|a| a == "acog") {
            "acog"
        } else {
            ""
        };
        let spec = format!("{weapon}:{combo}");
        for camo in [6, platinum::CAMO, platinum::DIAMOND] {
            for field in [GUN_MODEL, WORLD_MODEL] {
                let owner = world.spawn_empty().id();
                let mut skeleton = Skeleton::default();
                let mut queue = CommandQueue::default();
                let mut commands = Commands::new(&mut queue, &world);
                let mut assets = GunAssets {
                    meshes: &mut meshes,
                    materials: &mut materials,
                    images: &mut images,
                    bindposes: &mut bindposes,
                    camo_materials: &mut camo_materials,
                };
                let spawned = spawn_model_of(
                    &mut commands,
                    &mut content,
                    &mut camos,
                    &mut assets,
                    &mut skeleton,
                    &spec,
                    camo,
                    GunTarget { owner, attach_to: None, layers: None },
                    field,
                );
                assert!(spawned.is_some(), "{spec} camo {camo} {field}: composed model must spawn");
                let ((lo, hi), entities) = spawned.unwrap();
                assert!(lo.is_finite() && hi.is_finite() && hi.cmpgt(lo).any(), "{spec}: finite preview bounds");
                assert!(!entities.is_empty() && !skeleton.joints.is_empty(), "{spec}: skinned surfaces required");
                queue.apply(&mut world);
                if camo == platinum::DIAMOND {
                    let mut crowns = 0;
                    for entity in entities {
                        let Some(material) = world.get::<MeshMaterial3d<CamoMaterial>>(entity) else { continue };
                        let material = camo_materials.get(&material.0).unwrap();
                        if material.extension.scale.z < 4.5 {
                            continue;
                        }
                        let mesh = meshes.get(&world.get::<Mesh3d>(entity).unwrap().0).unwrap();
                        let Some(VertexAttributeValues::Float32x2(tags)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1) else {
                            panic!("{spec} {field}: every Diamond-coated drawn surface needs raised crowns")
                        };
                        let marked = tags.iter().filter(|uv| uv[0] > 0.5).count();
                        assert!(marked > 0 && marked.is_multiple_of(41), "{spec} {field}: actual complete crowns");
                        crowns += marked / 41;
                    }
                    assert!(
                        (1..=2400).contains(&crowns),
                        "{spec} {field}: visible crowns within model-wide budget, got {crowns}"
                    );
                }
                assert!(world.despawn(owner));
            }
        }
    }
    assert!(raised_surfaces >= 56, "at least first-person/world coating for each firearm");
    println!(
        "Checked {} native gun entries, {checked_models} model/finish variants and {raised_surfaces} raised diamond surfaces",
        guns.len()
    );
    Ok(())
}

fn check_raised_mesh(source: &Mesh, result: &Mesh, bones: usize, context: &str) {
    let old = source.count_vertices();
    let VertexAttributeValues::Float32x3(positions) = result.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else {
        panic!()
    };
    let VertexAttributeValues::Float32x3(normals) = result.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() else { panic!() };
    let VertexAttributeValues::Float32x2(tags) = result.attribute(Mesh::ATTRIBUTE_UV_1).unwrap() else { panic!() };
    let VertexAttributeValues::Uint16x4(joints) = result.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).unwrap() else {
        panic!()
    };
    let VertexAttributeValues::Float32x4(weights) = result.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT).unwrap() else {
        panic!()
    };
    assert!(positions.len() > old, "{context}: actual raised points required");
    assert_eq!((positions.len() - old) % 41, 0, "{context}: whole eight-facet crowns");
    assert!((positions.len() - old) / 41 <= diamond::MAX_STUDS, "{context}: bounded geometry");
    assert!(tags[..old].iter().all(|uv| *uv == [0.0; 2]), "{context}: source backing tag");
    assert!(tags[old..].iter().all(|uv| *uv == [1.0; 2]), "{context}: crown tag");
    assert!(positions.iter().all(|p| Vec3::from_array(*p).is_finite()), "{context}: finite positions");
    assert!(normals.iter().all(|n| Vec3::from_array(*n).is_finite()), "{context}: finite normals");
    assert!(result.indices().unwrap().iter().all(|i| i < positions.len()), "{context}: valid indices");
    for (_, values) in result.attributes() {
        assert_eq!(values.len(), positions.len(), "{context}: complete vertex streams");
    }
    for (j, w) in joints[old..].iter().zip(&weights[old..]) {
        assert!(j.iter().all(|j| (*j as usize) < bones), "{context}: native bone indices");
        assert!(w.iter().all(|w| w.is_finite() && *w >= 0.0), "{context}: valid inherited skin weights");
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{context}: normalized skin weights");
    }
    for stud in positions[old..].chunks_exact(41) {
        let base = (0..8).map(|i| Vec3::from_array(stud[i * 4])).sum::<Vec3>() / 8.0;
        let table = Vec3::from_array(stud[32]);
        assert!(base.distance(table) > 0.001, "{context}: table must rise by over one millimetre");
    }
    let old_bounds = source.compute_aabb().unwrap();
    let new_bounds = result.compute_aabb().unwrap();
    let old_min: Vec3 = old_bounds.min().into();
    let old_max: Vec3 = old_bounds.max().into();
    let new_min: Vec3 = new_bounds.min().into();
    let new_max: Vec3 = new_bounds.max().into();
    assert!(
        new_min.cmple(old_min + Vec3::splat(1e-6)).all() && new_max.cmpge(old_max - Vec3::splat(1e-6)).all(),
        "{context}: raised mesh bounds must retain all original gun geometry"
    );
}
