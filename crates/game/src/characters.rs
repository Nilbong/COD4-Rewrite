//! Characters the player can wear, collected from supply drops
//! ([`crate::supply`]): soldiers of CoD4 and of Black Ops as each game's own
//! character scripts put them together (a body, maybe a head, and in Black
//! Ops hats and gear), multiplayer and campaign alike. Their models live in
//! the zone named here; `COD4RW_GALLERY` renders them all
//! ([`crate::gallery`]). Black Ops' come from its install through the `t5`
//! crate, and some use materials another of its levels defines, loaded
//! alongside (`also`).
//!
//! Everyone starts as [`DEFAULT`], a Spetsnaz rifleman. Enlisted are the
//! other soldiers, Veteran the ghillie suits and other rarer outfits,
//! Professional the campaigns' supporting cast, Elite their leads.

use crate::supply::Rarity;

/// The game a character comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    Cod4,
    BlackOps,
}

impl Game {
    pub fn name(self) -> &'static str {
        match self {
            Game::Cod4 => "CALL OF DUTY 4",
            Game::BlackOps => "BLACK OPS",
        }
    }

    /// For tight spaces, like the character list's rows.
    pub fn short(self) -> &'static str {
        match self {
            Game::Cod4 => "COD4",
            Game::BlackOps => "BO",
        }
    }
}

pub struct Character {
    /// What the collection file stores.
    pub id: &'static str,
    pub name: &'static str,
    pub rarity: Rarity,
    pub game: Game,
    /// The zone holding its models.
    pub zone: &'static str,
    /// The body, then what is attached to it (head, hat, gear).
    pub models: &'static [&'static str],
    /// Other zones whose materials it uses (Black Ops).
    pub also: &'static [&'static str],
}

/// A CoD4 character.
const fn c(id: &'static str, name: &'static str, rarity: Rarity, zone: &'static str, models: &'static [&'static str]) -> Character {
    Character { id, name, rarity, game: Game::Cod4, zone, models, also: &[] }
}

/// A Black Ops character.
const fn b(
    id: &'static str,
    name: &'static str,
    rarity: Rarity,
    zone: &'static str,
    models: &'static [&'static str],
    also: &'static [&'static str],
) -> Character {
    Character { id, name, rarity, game: Game::BlackOps, zone, models, also }
}

use Rarity::{Elite as L, Enlisted as E, Professional as P, Veteran as V};

/// The starting character, always owned.
pub const DEFAULT: &str = "spetsnaz";

pub static CHARACTERS: &[Character] = &[
    c("spetsnaz", "Spetsnaz", E, "mp_killhouse", &["body_mp_opforce_assault", "head_mp_opforce_headwrap"]),
    // Multiplayer.
    c("spetsnaz_cqb", "Spetsnaz CQB", E, "mp_killhouse", &["body_mp_opforce_cqb", "head_mp_opforce_gasmask"]),
    c("spetsnaz_engineer", "Spetsnaz Engineer", E, "mp_killhouse", &["body_mp_opforce_eningeer", "head_mp_opforce_gasmask"]),
    c("spetsnaz_support", "Spetsnaz Support", E, "mp_killhouse", &["body_mp_opforce_support", "head_mp_opforce_3hole_mask"]),
    c("spetsnaz_urban_sniper", "Spetsnaz Urban Sniper", E, "mp_killhouse", &["body_mp_opforce_sniper_urban", "head_mp_opforce_justin"]),
    c("sas_assault", "SAS Assault", E, "mp_killhouse", &["body_mp_sas_urban_assault"]),
    c("sas_recon", "SAS Recon", E, "mp_killhouse", &["body_mp_sas_urban_recon"]),
    c("sas_sniper", "SAS Sniper", E, "mp_killhouse", &["body_mp_sas_urban_sniper"]),
    c("sas_specops", "SAS Spec Ops", E, "mp_killhouse", &["body_mp_sas_urban_specops"]),
    c("sas_support", "SAS Support", E, "mp_killhouse", &["body_mp_sas_urban_support"]),
    c("marine_assault", "Marine Assault", E, "mp_crash", &["body_mp_usmc_assault", "head_mp_usmc_tactical_mich"]),
    c("marine_recon", "Marine Recon", E, "mp_crash", &["body_mp_usmc_recon", "head_mp_usmc_nomex"]),
    c("marine_sniper", "Marine Sniper", E, "mp_crash", &["body_mp_usmc_sniper", "head_mp_usmc_tactical_baseball_cap"]),
    c("marine_specops", "Marine Spec Ops", E, "mp_crash", &["body_mp_usmc_specops", "head_mp_usmc_tactical_mich_stripes_nomex"]),
    c("marine_support", "Marine Support", E, "mp_crash", &["body_mp_usmc_support", "head_mp_usmc_shaved_head"]),
    c("woodland_assault", "Woodland Marine Assault", E, "mp_bloc", &["body_mp_usmc_woodland_assault", "head_mp_usmc_tactical_mich"]),
    c("woodland_recon", "Woodland Marine Recon", E, "mp_bloc", &["body_mp_usmc_woodland_recon", "head_mp_usmc_nomex"]),
    c("woodland_specops", "Woodland Marine Spec Ops", E, "mp_bloc", &["body_mp_usmc_woodland_specops", "head_mp_usmc_tactical_mich_stripes_nomex"]),
    c("woodland_support", "Woodland Marine Support", E, "mp_bloc", &["body_mp_usmc_woodland_support", "head_mp_usmc_shaved_head"]),
    c("opfor_assault", "OpFor Assault", E, "mp_crash", &["body_mp_arab_regular_assault", "head_mp_arab_regular_suren"]),
    c("opfor_cqb", "OpFor CQB", E, "mp_crash", &["body_mp_arab_regular_cqb", "head_mp_arab_regular_headwrap"]),
    c("opfor_engineer", "OpFor Engineer", E, "mp_crash", &["body_mp_arab_regular_engineer", "head_mp_arab_regular_ski_mask"]),
    c("opfor_sniper", "OpFor Sniper", E, "mp_crash", &["body_mp_arab_regular_sniper", "head_mp_arab_regular_sadiq"]),
    c("opfor_support", "OpFor Support", E, "mp_crash", &["body_mp_arab_regular_support", "head_mp_arab_regular_asad"]),
    // Campaign soldiers.
    c("sas_ct_benjamin", "SAS CT Benjamin", E, "killhouse", &["body_complete_sp_sas_ct_benjamin"]),
    c("sas_ct_charles", "SAS CT Charles", E, "killhouse", &["body_complete_sp_sas_ct_charles"]),
    c("sas_ct_mitchel", "SAS CT Mitchel", E, "killhouse", &["body_complete_sp_sas_ct_mitchel"]),
    c("sas_ct_neal", "SAS CT Neal", E, "killhouse", &["body_complete_sp_sas_ct_neal"]),
    c("sas_ct_william", "SAS CT William", E, "killhouse", &["body_complete_sp_sas_ct_william"]),
    c("sas_hugh", "SAS Hugh", E, "killhouse", &["body_sp_sas_woodland_support_a", "head_sp_sas_woodland_hugh"]),
    c("sas_zied", "SAS Zied", E, "killhouse", &["body_sp_sas_woodland_assault_a", "head_sp_sas_woodland_zied"]),
    c("sas_peter", "SAS Peter", E, "killhouse", &["body_sp_sas_woodland_assault_a", "head_sp_sas_woodland_peter"]),
    c("sas_todd", "SAS Todd", E, "killhouse", &["body_sp_sas_woodland_assault_a", "head_sp_sas_woodland_todd"]),
    c("spetsnaz_boris", "Spetsnaz Boris", E, "cargoship", &["body_sp_spetsnaz_boris", "head_sp_spetsnaz_boris_borisbody"]),
    c("spetsnaz_demetry", "Spetsnaz Demetry", E, "cargoship", &["body_sp_spetsnaz_demetry", "head_sp_spetsnaz_demetry_demetrybody"]),
    c("spetsnaz_vlad", "Spetsnaz Vlad", E, "cargoship", &["body_sp_spetsnaz_vlad", "head_sp_spetsnaz_vlad_vladbody"]),
    c("spetsnaz_yuri", "Spetsnaz Yuri", E, "cargoship", &["body_sp_spetsnaz_yuri", "head_sp_spetsnaz_yuri_yuribody"]),
    c("spetsnaz_collins", "Spetsnaz Collins", E, "cargoship", &["body_sp_spetsnaz_vlad", "head_sp_spetsnaz_collins_vladbody"]),
    c("spetsnaz_geoff", "Spetsnaz Geoff", E, "cargoship", &["body_sp_spetsnaz_yuri", "head_sp_spetsnaz_geoff_yuribody"]),
    c("spetsnaz_derik", "Spetsnaz Derik", E, "cargoship", &["body_sp_spetsnaz_boris", "head_sp_spetsnaz_derik_borisbody"]),
    c("loyalist_alex", "Loyalist Alex", E, "blackout", &["body_sp_russian_loyalist_a", "head_sp_loyalist_alex_helmet_body_a"]),
    c("loyalist_mackey", "Loyalist Mackey", E, "blackout", &["body_sp_russian_loyalist_b", "head_sp_loyalist_mackey_hat_body_b"]),
    c("loyalist_josh", "Loyalist Josh", E, "blackout", &["body_sp_russian_loyalist_c", "head_sp_loyalist_josh_helmet_body_c"]),
    c("loyalist_tom", "Loyalist Tom", E, "blackout", &["body_sp_russian_loyalist_d", "head_sp_loyalist_tom_hat_body_d"]),
    c("ultranat_ski_mask", "Ultranationalist Ski Mask", E, "village_assault", &["body_sp_opforce_a", "head_sp_opforce_ski_mask_body_a"]),
    c("ultranat_balaclava", "Ultranationalist Balaclava", E, "hunted", &["body_sp_opforce_b", "head_sp_opforce_3hole_ski_mask_body_b"]),
    c("ultranat_david", "Ultranationalist David", E, "hunted", &["body_sp_opforce_c", "head_sp_opforce_david_beanie_body_c"]),
    c("ultranat_wrap", "Ultranationalist Wrap", E, "hunted", &["body_sp_opforce_d", "head_sp_opforce_fullwrap_body_d"]),
    c("ultranat_justin", "Ultranationalist Justin", E, "hunted", &["body_sp_opforce_e", "head_sp_opforce_justin_beanie_body_e"]),
    c("ultranat_gas_mask", "Ultranationalist Gas Mask", E, "hunted", &["body_sp_opforce_f", "head_sp_opforce_gas_mask_body_f"]),
    c("ultranat_collins", "Ultranationalist Collins", E, "hunted", &["body_sp_opforce_d", "head_sp_opforce_collins_headset_body_d"]),
    c("ultranat_geoff", "Ultranationalist Geoff", E, "hunted", &["body_sp_opforce_c", "head_sp_opforce_geoff_headset_body_c"]),
    c("ultranat_derik", "Ultranationalist Derik", E, "hunted", &["body_sp_opforce_f", "head_sp_opforce_derik_body_f"]),
    c("opfor_sadiq", "OpFor Sadiq", E, "coup", &["body_sp_arab_regular_sadiq", "head_sp_arab_regular_sadiq"]),
    c("opfor_ski_mask", "OpFor Ski Mask", E, "coup", &["body_sp_arab_regular_coup_b", "head_sp_arab_regular_ski_mask"]),
    c("opfor_asad", "OpFor Asad", E, "coup", &["body_sp_arab_regular_asad", "head_sp_arab_regular_asad"]),
    c("opfor_suren", "OpFor Suren", E, "coup", &["body_sp_arab_regular_suren", "head_sp_arab_regular_suren"]),
    c("opfor_yasir", "OpFor Yasir", E, "coup", &["body_sp_arab_regular_yasir", "head_sp_arab_regular_mowrap"]),
    c("opfor_tariq", "OpFor Tariq", E, "bog_a", &["body_sp_arab_regular_tariq", "head_sp_arab_regular_suren"]),
    c("marine_james", "Marine James", E, "armada", &["body_sp_usmc_james", "head_sp_usmc_james_james_body"]),
    c("marine_sami", "Marine Sami", E, "armada", &["body_sp_usmc_james", "head_sp_usmc_sami_zach_body"]),
    c("marine_sami_goggles", "Marine Sami Goggles", E, "armada", &["body_sp_usmc_james", "head_sp_usmc_sami_goggles_zach_body"]),
    c("marine_at4", "Marine AT4", E, "armada", &["body_sp_usmc_at4", "head_sp_usmc_james_james_body"]),
    c("marine_ryan", "Marine Ryan", E, "armada", &["body_sp_usmc_ryan", "head_sp_usmc_ryan_ryan_body"]),
    c("marine_zach", "Marine Zach", E, "armada", &["body_sp_usmc_zach", "head_sp_usmc_zach_zach_body"]),
    c("marine_zach_goggles", "Marine Zach Goggles", E, "armada", &["body_sp_usmc_zach", "head_sp_usmc_zach_zach_body_goggles"]),
    c("force_recon_a", "Force Recon", E, "sniperescape", &["body_sp_usmc_force_a", "head_sp_usmc_force_nomex"]),
    c("force_recon_chad", "Force Recon Chad", E, "sniperescape", &["body_sp_usmc_force_b", "head_sp_usmc_force_chad"]),
    c("force_recon_c", "Force Recon Gunner", E, "sniperescape", &["body_sp_usmc_force_c", "head_sp_usmc_force_nomex"]),
    // Veteran: ghillie suits and other rarer outfits.
    c("marine_ghillie", "Marine Ghillie", V, "mp_bloc", &["body_mp_usmc_woodland_sniper", "head_mp_usmc_ghillie"]),
    c("spetsnaz_ghillie", "Spetsnaz Ghillie", V, "mp_bloc", &["body_mp_opforce_sniper", "head_mp_opforce_ghillie"]),
    c("ghillie_price", "Ghillie Price", V, "scoutsniper", &["body_complete_sp_usmc_ghillie_price"]),
    c("marine_nvg_james", "Marine NVG James", V, "bog_a", &["body_sp_usmc_james", "head_sp_usmc_james_james_body_nod"]),
    c("marine_nvg_sami", "Marine NVG Sami", V, "bog_a", &["body_sp_usmc_zach", "head_sp_usmc_sami_zach_body_nod"]),
    c("marine_nvg_zach", "Marine NVG Zach", V, "bog_a", &["body_sp_usmc_zach", "head_sp_usmc_zach_zach_body_nod"]),
    c("marine_nvg_ryan", "Marine NVG Ryan", V, "bog_a", &["body_sp_usmc_ryan", "head_sp_usmc_ryan_ryan_body_nod"]),
    c("pilot_velinda", "Cobra Pilot Velinda", V, "bog_a", &["body_complete_sp_cobra_pilot_desert_velinda"]),
    c("pilot_zack", "Cobra Pilot Zack", V, "bog_a", &["body_complete_sp_cobra_pilot_desert_zack"]),
    c("pilot_velinda_woodland", "Woodland Pilot Velinda", V, "cargoship", &["body_complete_sp_cobra_pilot_woodland_velinda"]),
    c("pilot_zack_woodland", "Woodland Pilot Zack", V, "cargoship", &["body_complete_sp_cobra_pilot_woodland_zack"]),
    c("russian_farmer", "Russian Farmer", V, "hunted", &["body_complete_sp_russian_farmer"]),
    // Professional: the supporting cast.
    c("nikolai", "Nikolai", P, "blackout", &["body_complete_sp_vip"]),
    c("griggs", "Griggs", P, "bog_a", &["body_complete_sp_usmc_mark"]),
    c("griggs_nvg", "Griggs NVG", P, "bog_a", &["body_complete_sp_usmc_mark_nod"]),
    c("griggs_force_recon", "Griggs Force Recon", P, "village_defend", &["body_complete_sp_usmc_force_mark"]),
    c("griggs_no_helmet", "Griggs No Helmet", P, "jeepride", &["sp_usmc_force_mark_no_helmet"]),
    c("vasquez", "Lt. Vasquez", P, "bog_a", &["body_complete_sp_usmc_vasquez"]),
    c("mac", "Mac", P, "killhouse", &["body_sp_sas_woodland_assault_b", "head_sp_sas_woodland_mac"]),
    c("victor_zakhaev", "Victor Zakhaev", P, "ambush", &["body_complete_sp_zakhaevs_son"]),
    c("victor_zakhaev_tracksuit", "Victor Zakhaev Tracksuit", P, "coup", &["body_complete_sp_zakhaevs_son_coup"]),
    // Elite: the leads.
    c("price", "Captain Price", L, "cargoship", &["body_complete_sp_sas_ct_price"]),
    c("price_mask_up", "Captain Price Mask Up", L, "killhouse", &["body_complete_sp_sas_ct_price_maskup"]),
    c("price_nvg", "Captain Price NVG", L, "blackout", &["body_complete_sp_sas_nvg_price"]),
    c("price_woodland", "Captain Price Woodland", L, "hunted", &["body_complete_sp_sas_woodland_price"]),
    c("gaz", "Gaz", L, "killhouse", &["body_complete_sp_sas_woodland_gaz"]),
    c("zakhaev", "Imran Zakhaev", L, "sniperescape", &["body_complete_sp_zakhaev"]),
    c("zakhaev_one_arm", "Imran Zakhaev One Arm", L, "sniperescape", &["body_complete_onearm_sp_zakhaev"]),
    c("zakhaev_wounded", "Imran Zakhaev Wounded", L, "coup", &["body_complete_gimp_sp_zakhaev"]),
    c("al_asad", "Khaled Al-Asad", L, "coup", &["body_complete_sp_arab_regular_al_asad"]),
    c("al_asad_captured", "Khaled Al-Asad Captured", L, "village_assault", &["body_complete_sp_arab_regular_al_asad_dmg"]),
    c("vip", "The VIP", L, "airplane", &["body_complete_sp_vip_pres"]),
    // Black Ops multiplayer: each faction's five looks (camo is Veteran).
    b("bo_rus_spet_armor", "Spetsnaz Armor", E, "mp_nuked", &["c_rus_spet_mp_body_armor", "c_rus_spet_mp_head_1"], &[]),
    b("bo_rus_spet_camo", "Spetsnaz Camo", V, "mp_nuked", &["c_rus_spet_mp_body_camo", "c_rus_spet_mp_head_2"], &[]),
    b("bo_rus_spet_flak", "Spetsnaz Flak", E, "mp_nuked", &["c_rus_spet_mp_body_flak", "c_rus_spet_mp_head_3"], &[]),
    b("bo_rus_spet_standard", "Spetsnaz Standard", E, "mp_nuked", &["c_rus_spet_mp_body_standard", "c_rus_spet_mp_head_4"], &[]),
    b("bo_rus_spet_utility", "Spetsnaz Utility", E, "mp_nuked", &["c_rus_spet_mp_body_utility", "c_rus_spet_mp_head_5"], &[]),
    b("bo_usa_cia_armor", "CIA Armor", E, "mp_nuked", &["c_usa_cia_mp_body_armor", "c_usa_cia_mp_head_1"], &[]),
    b("bo_usa_cia_camo", "CIA Camo", V, "mp_nuked", &["c_usa_cia_mp_body_camo", "c_usa_cia_mp_head_2"], &[]),
    b("bo_usa_cia_flak", "CIA Flak", E, "mp_nuked", &["c_usa_cia_mp_body_flak", "c_usa_cia_mp_head_3"], &[]),
    b("bo_usa_cia_standard", "CIA Standard", E, "mp_nuked", &["c_usa_cia_mp_body_standard", "c_usa_cia_mp_head_4"], &[]),
    b("bo_usa_cia_utility", "CIA Utility", E, "mp_nuked", &["c_usa_cia_mp_body_utility", "c_usa_cia_mp_head_5"], &[]),
    b("bo_rus_spetwin_armor", "Winter Spetsnaz Armor", E, "mp_array", &["c_rus_spetwin_mp_body_armor", "c_rus_spetwin_mp_head_1"], &[]),
    b("bo_rus_spetwin_camo", "Winter Spetsnaz Camo", V, "mp_array", &["c_rus_spetwin_mp_body_camo", "c_rus_spetwin_mp_head_2"], &[]),
    b("bo_rus_spetwin_flak", "Winter Spetsnaz Flak", E, "mp_array", &["c_rus_spetwin_mp_body_flak", "c_rus_spetwin_mp_head_3"], &[]),
    b("bo_rus_spetwin_standard", "Winter Spetsnaz Standard", E, "mp_array", &["c_rus_spetwin_mp_body_standard", "c_rus_spetwin_mp_head_4"], &[]),
    b("bo_rus_spetwin_utility", "Winter Spetsnaz Utility", E, "mp_array", &["c_rus_spetwin_mp_body_utility", "c_rus_spetwin_mp_head_5"], &[]),
    b("bo_usa_ciawin_armor", "Winter CIA Armor", E, "mp_array", &["c_usa_ciawin_mp_body_armor", "c_usa_ciawin_mp_head_1"], &[]),
    b("bo_usa_ciawin_camo", "Winter CIA Camo", V, "mp_array", &["c_usa_ciawin_mp_body_camo", "c_usa_ciawin_mp_head_2"], &[]),
    b("bo_usa_ciawin_flak", "Winter CIA Flak", E, "mp_array", &["c_usa_ciawin_mp_body_flak", "c_usa_ciawin_mp_head_3"], &[]),
    b("bo_usa_ciawin_standard", "Winter CIA Standard", E, "mp_array", &["c_usa_ciawin_mp_body_standard", "c_usa_ciawin_mp_head_4"], &[]),
    b("bo_usa_ciawin_utility", "Winter CIA Utility", E, "mp_array", &["c_usa_ciawin_mp_body_utility", "c_usa_ciawin_mp_head_5"], &[]),
    b("bo_usa_sog_armor", "SOG Armor", E, "mp_cracked", &["c_usa_sog_mp_body_armor", "c_usa_sog_mp_head_1"], &[]),
    b("bo_usa_sog_camo", "SOG Ghillie", V, "mp_cracked", &["c_usa_sog_mp_body_camo", "c_usa_sog_mp_head_2"], &[]),
    b("bo_usa_sog_flak", "SOG Flak", E, "mp_cracked", &["c_usa_sog_mp_body_flak", "c_usa_sog_mp_head_3"], &[]),
    b("bo_usa_sog_standard", "SOG Standard", E, "mp_cracked", &["c_usa_sog_mp_body_standard", "c_usa_sog_mp_head_4"], &[]),
    b("bo_usa_sog_utility", "SOG Utility", E, "mp_cracked", &["c_usa_sog_mp_body_utility", "c_usa_sog_mp_head_5"], &[]),
    b("bo_vtn_nva_armor", "NVA Armor", E, "mp_cracked", &["c_vtn_nva_mp_body_armor", "c_vtn_nva_mp_head_1"], &[]),
    b("bo_vtn_nva_camo", "NVA Ghillie", V, "mp_cracked", &["c_vtn_nva_mp_body_camo", "c_vtn_nva_mp_head_2"], &[]),
    b("bo_vtn_nva_flak", "NVA Flak", E, "mp_cracked", &["c_vtn_nva_mp_body_flak", "c_vtn_nva_mp_head_3"], &[]),
    b("bo_vtn_nva_standard", "NVA Standard", E, "mp_cracked", &["c_vtn_nva_mp_body_standard", "c_vtn_nva_mp_head_4"], &[]),
    b("bo_vtn_nva_utility", "NVA Utility", E, "mp_cracked", &["c_vtn_nva_mp_body_utility", "c_vtn_nva_mp_head_5"], &[]),
    b("bo_cub_rebels_armor", "Cuban Rebel Armor", E, "mp_firingrange", &["c_cub_rebels_mp_body_armor", "c_cub_rebels_mp_head_1"], &[]),
    b("bo_cub_rebels_camo", "Cuban Rebel Camo", V, "mp_firingrange", &["c_cub_rebels_mp_body_camo", "c_cub_rebels_mp_head_2"], &[]),
    b("bo_cub_rebels_flak", "Cuban Rebel Flak", E, "mp_firingrange", &["c_cub_rebels_mp_body_flak", "c_cub_rebels_mp_head_3"], &[]),
    b("bo_cub_rebels_standard", "Cuban Rebel Standard", E, "mp_firingrange", &["c_cub_rebels_mp_body_standard", "c_cub_rebels_mp_head_4"], &[]),
    b("bo_cub_rebels_utility", "Cuban Rebel Utility", E, "mp_firingrange", &["c_cub_rebels_mp_body_utility", "c_cub_rebels_mp_head_5"], &[]),
    b("bo_cub_tropas_armor", "Tropas Armor", E, "mp_firingrange", &["c_cub_tropas_mp_body_armor", "c_cub_tropas_mp_head_1"], &[]),
    b("bo_cub_tropas_camo", "Tropas Camo", V, "mp_firingrange", &["c_cub_tropas_mp_body_camo", "c_cub_tropas_mp_head_2"], &[]),
    b("bo_cub_tropas_flak", "Tropas Flak", E, "mp_firingrange", &["c_cub_tropas_mp_body_flak", "c_cub_tropas_mp_head_3"], &[]),
    b("bo_cub_tropas_standard", "Tropas Standard", E, "mp_firingrange", &["c_cub_tropas_mp_body_standard", "c_cub_tropas_mp_head_4"], &[]),
    b("bo_cub_tropas_utility", "Tropas Utility", E, "mp_firingrange", &["c_cub_tropas_mp_body_utility", "c_cub_tropas_mp_head_5"], &[]),
    // Black Ops campaign soldiers: one of each look.
    b("bo_tropas", "Cuban Army", E, "cuba", &["c_cub_tropas_body1", "c_cub_tropas_head1", "c_cub_tropas_gear1"], &[]),
    b("bo_cuban_rebel", "Bay of Pigs Rebel", E, "cuba", &["c_cub_rebels_body1", "c_cub_rebels_head1", "c_cub_rebels_hat1"], &[]),
    b("bo_cuban_police", "Cuban Police", E, "cuba", &["c_cub_police_body", "c_cub_police_head1", "c_cub_police_hat", "c_cub_police_gear"], &[]),
    b("bo_cia_agent", "CIA Agent", E, "pentagon", &["c_usa_pent_ciaagent_body", "c_usa_pent_ciaagent_head2", "c_usa_pent_ciaagent_radio"], &[]),
    b("bo_military_police", "Military Police", E, "pentagon", &["c_usa_militarypolice_body", "c_usa_militarypolice_head1", "c_usa_militarypolice_hat"], &[]),
    b("bo_general", "Pentagon General", E, "pentagon", &["c_usa_pent_general_body", "c_usa_pent_general_head"], &[]),
    b("bo_soviet_soldier", "Soviet Soldier", E, "flashpoint", &["c_rus_military_body1", "c_rus_military_head1", "c_rus_military_body1_gear1"], &[]),
    b("bo_spetsnaz_balaclava", "Spetsnaz Balaclava", E, "flashpoint", &["c_rus_spetznaz_body1", "c_rus_spetznaz_body1_head1", "c_rus_spetznaz_body1_gear1"], &[]),
    b("bo_spetsnaz_woodland", "Spetsnaz Woodland", E, "flashpoint", &["c_rus_spetznaz_body2", "c_rus_spetznaz_body2_head1", "c_rus_spetznaz_body2_gear1"], &[]),
    b("bo_nva_regular", "NVA Regular", E, "khe_sanh", &["c_vtn_nva1_body", "c_vtn_nva1_head", "c_vtn_nva1_gear"], &["mp_nuked"]),
    b("bo_nva_pith", "NVA Pith Helmet", E, "khe_sanh", &["c_vtn_nva2_body", "c_vtn_nva2_head", "c_vtn_nva2_gear"], &["mp_nuked"]),
    b("bo_jungle_marine", "Jungle Marine", E, "creek_1", &["c_usa_jungmar_wet_body", "c_usa_jungmar_wet_head1", "c_usa_jungmar_wet_gear1"], &[]),
    b("bo_jungle_marine_sniper", "Jungle Marine Sniper", E, "creek_1", &["c_usa_jungmar_wet_body", "c_usa_jungmar_wet_head5", "c_usa_jungmar_wet_gear5"], &[]),
    b("bo_viet_cong", "Viet Cong", E, "creek_1", &["c_vtn_vc1_body", "c_vtn_vc1_head", "c_vtn_vc1_gear"], &[]),
    b("bo_viet_cong_grass", "Viet Cong Grass Hat", E, "creek_1", &["c_vtn_vc2_body", "c_vtn_vc2_head", "c_vtn_vc2_gear"], &[]),
    b("bo_vc_bomber", "Viet Cong Bomber", E, "pow", &["c_vtn_vc_bomber_body", "c_vtn_vc_bomber_head"], &[]),
    b("bo_soviet_winter", "Soviet Winter", E, "wmd_sr71", &["c_rus_military_winter_body1", "c_rus_military_winter_body1_head1", "c_rus_military_winter_body1_gear1"], &[]),
    b("bo_spetsnaz_winter", "Spetsnaz Winter", E, "wmd_sr71", &["c_rus_spetznaz_winter_body1", "c_rus_spetznaz_winter_head1", "c_rus_spetznaz_winter_body1_gear1"], &[]),
    b("bo_spetsnaz_snow", "Spetsnaz Snow", E, "wmd_sr71", &["c_rus_spetznaz_winter_body3", "c_rus_spetznaz_winter_head3", "c_rus_spetznaz_winter_body3_gear1"], &[]),
    b("bo_black_ops_winter", "Black Ops Winter", E, "wmd_sr71", &["c_usa_blackops_winter_body2_fb"], &[]),
    b("bo_black_ops", "Black Ops Operative", E, "kowloon", &["c_usa_blackops_body2_fb"], &[]),
    b("bo_black_ops_cap", "Black Ops Cap", E, "kowloon", &["c_usa_blackops_body3_fb"], &[]),
    b("bo_sog_operative", "SOG Operative", E, "underwaterbase", &["c_usa_ubase_body1", "c_usa_ubase_head1", "c_usa_ubase_hat1", "c_usa_ubase_gear_combat"], &[]),
    b("bo_wehrmacht", "Wehrmacht", E, "fullahead", &["c_ger_infantry_body", "c_ger_infantry_head1", "c_ger_infantry_helm", "c_ger_infantry_gear"], &[]),
    b("bo_british_commando", "British Commando", E, "fullahead", &["c_brt_fullahead_soldier_body", "c_brt_fullahead_soldier_head", "c_brt_fullahead_soldier_gear"], &[]),
    b("bo_red_army", "Red Army", E, "fullahead", &["c_rus_fullahead_soldier_body1", "c_rus_fullahead_head1"], &[]),
    b("bo_red_army_officer", "Red Army Officer", E, "fullahead", &["c_rus_fullahead_officer1_body", "c_rus_fullahead_officer1_head"], &[]),
    // Black Ops Veteran: rarer outfits.
    b("bo_hazmat", "Hazmat Suit", V, "rebirth", &["c_usa_rebirth_hazmat_body", "c_usa_rebirth_hazmat_head", "c_usa_rebirth_hazmat_mask"], &["mp_nuked"]),
    b("bo_soviet_hazmat", "Soviet Hazmat", V, "rebirth", &["c_rus_hazmat_fb"], &[]),
    b("bo_rebirth_engineer", "Rebirth Engineer", V, "rebirth", &["c_rus_engineer1_body_orange", "c_rus_engineer_head1", "c_rus_engineer_helmet", "c_rus_engineer_headgear1"], &["mp_nuked"]),
    b("bo_sog_diver", "SOG Diver", V, "underwaterbase", &["c_usa_ubase_body1", "c_usa_ubase_head1", "c_usa_ubase_flippers", "c_usa_ubase_gear1"], &[]),
    b("bo_soviet_heavy", "Soviet Heavy", V, "underwaterbase", &["c_rus_heavy_fb"], &[]),
    b("bo_huey_pilot", "Huey Pilot", V, "khe_sanh", &["c_usa_huey_pilot_body", "c_usa_huey_pilot_head1", "c_usa_huey_pilot_helmet_logo"], &["mp_nuked"]),
    b("bo_halo_jumper", "HALO Jumper", V, "wmd_sr71", &["c_usa_blackops_winter_body1_halo_fb"], &[]),
    b("bo_spetsnaz_gas_mask", "Spetsnaz Gas Mask", V, "flashpoint", &["c_rus_spetznaz_body1", "c_rus_spetznaz_body1_head1_gasmask", "c_rus_spetznaz_body1_gear1"], &[]),
    b("bo_frozen_wehrmacht", "Frozen Wehrmacht", V, "fullahead", &["c_ger_infantry_frozen_body", "c_ger_infantry_frozen_head1", "c_ger_infantry_frozen_helm", "c_ger_infantry_frozen_gear"], &[]),
    // Black Ops Professional: the supporting cast.
    b("hudson", "Hudson", P, "pentagon", &["c_usa_pentagon_hudson_fb"], &[]),
    b("hudson_rusalka", "Hudson Rusalka", P, "underwaterbase", &["c_usa_ubase_hudson_fb", "c_usa_ubase_hudson_gear_combat"], &[]),
    b("bowman", "Bowman", P, "hue_city", &["c_usa_jungmar_bowman_fb", "c_usa_jungmar_bowman_gear"], &[]),
    b("bowman_cuba", "Bowman Cuba", P, "cuba", &["c_usa_cubrebel_bowman_fb"], &[]),
    b("weaver", "Weaver", P, "kowloon", &["c_usa_blackops_weaver_getwet_fb"], &[]),
    b("weaver_halo", "Weaver HALO", P, "wmd_sr71", &["c_usa_blackops_winter_weaver_halo_fb"], &[]),
    b("kravchenko", "Kravchenko", P, "cuba", &["c_rus_kravchenko_fb"], &[]),
    b("kravchenko_1945", "Kravchenko 1945", P, "fullahead", &["c_rus_kravchenko_young_fb"], &[]),
    b("steiner", "Steiner", P, "int_escape", &["c_ger_steiner_interrogation_fb"], &[]),
    b("steiner_1945", "Steiner 1945", P, "fullahead", &["c_ger_steiner_fullahead_fb"], &[]),
    b("carlos", "Carlos", P, "cuba", &["c_cub_carlos_battle_fb"], &[]),
    b("mcnamara", "McNamara", P, "pentagon", &["c_usa_pent_mcnamara_fb"], &[]),
    b("petrenko", "Petrenko", P, "fullahead", &["c_rus_fullahead_patrenko_fb"], &[]),
    // Black Ops Elite: the leads.
    b("mason", "Mason", L, "pentagon", &["c_usa_pentagon_mason_fb"], &[]),
    b("mason_interrogation", "Mason Interrogation", L, "int_escape", &["c_usa_interrogation_mason_fb"], &[]),
    b("mason_rebirth", "Mason Rebirth", L, "rebirth", &["c_usa_rebirth_mason_fb"], &["mp_nuked"]),
    b("woods", "Woods", L, "hue_city", &["c_usa_jungmar_barnes_fb", "c_usa_jungmar_barnes_bedroll"], &[]),
    b("woods_cuba", "Woods Cuba", L, "cuba", &["c_usa_cubrebel_barnes_fb"], &[]),
    b("woods_black_ops", "Woods Black Ops", L, "flashpoint", &["c_usa_specop_barnes_fb"], &[]),
    b("reznov", "Reznov", L, "creek_1", &["c_rus_reznov_combat_fb"], &[]),
    b("reznov_1945", "Reznov 1945", L, "fullahead", &["c_rus_reznov_fullahead_fb"], &[]),
    b("reznov_prisoner", "Reznov Prisoner", L, "fullahead", &["c_rus_reznov_prisoner_fb"], &[]),
    b("dragovich", "Dragovich", L, "cuba", &["c_rus_dragovich_old_fb"], &[]),
    b("dragovich_1945", "Dragovich 1945", L, "fullahead", &["c_rus_dragovich_young_fb"], &[]),
    b("jfk", "JFK", L, "pentagon", &["c_usa_pent_jfk_fb"], &[]),
    b("castro", "Castro", L, "cuba", &["c_cub_castro_jacket_fb"], &[]),
];

pub fn character(id: &str) -> Option<&'static Character> {
    CHARACTERS.iter().find(|c| c.id.eq_ignore_ascii_case(id))
}

/// The characters drops can hold: all but the starting one.
pub fn droppable(rarity: Rarity) -> impl Iterator<Item = &'static Character> {
    CHARACTERS.iter().filter(move |c| c.rarity == rarity && c.id != DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_every_rarity_has_some() {
        let mut ids: Vec<&str> = CHARACTERS.iter().map(|c| c.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), CHARACTERS.len());
        assert!(character(DEFAULT).is_some());
        for r in [Rarity::Enlisted, Rarity::Veteran, Rarity::Professional, Rarity::Elite] {
            assert!(droppable(r).count() > 0, "{r:?}");
        }
    }
}
