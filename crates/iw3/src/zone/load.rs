//! Per-asset stream loaders.
//!
//! Each loader mirrors the game's `Load_*` functions: read the struct, then
//! visit its pointer members in serialisation order, reading inline data for
//! `-1`/`-2` pointers. Offsets in comments are into the 32-bit struct image.

use super::reader::*;
use super::types::*;
use anyhow::{Result, bail};
use std::collections::HashMap;

pub(super) struct Loader<'a> {
    pub r: Reader<'a>,
    pub assets: Vec<Asset>,
    pub script_strings: Vec<String>,
    /// Material texture tables by block offset, so materials that share a
    /// table by reference still get resolved image handles.
    texture_tables: HashMap<(usize, u32), Vec<(u32, u8, u8, u8, u8, Option<AssetId>)>>,
}

/// Index of the record a reference points at, given the array's base pointer.
fn index_in(base: Option<(usize, u32)>, ptr: Ptr, size: u32) -> Option<u32> {
    let (bb, bo) = base?;
    match ptr {
        Ptr::Ref { block, offset } if block == bb && offset >= bo && (offset - bo) % size == 0 => {
            Some((offset - bo) / size)
        }
        _ => None,
    }
}

fn base_of(ptr: Ptr, loc: Option<Loc>) -> Option<(usize, u32)> {
    match (ptr, loc) {
        (Ptr::Ref { block, offset }, _) => Some((block, offset)),
        (_, Some(l)) => Some((l.block, l.offset)),
        _ => None,
    }
}

/// Alias key of a pointer field at `off` bytes into an allocation.
fn slot(loc: Loc, off: usize) -> Option<u32> {
    match loc.block {
        BLOCK_TEMP | BLOCK_RUNTIME | BLOCK_LARGE_RUNTIME | BLOCK_PHYSICAL_RUNTIME => None,
        b => Some(((b as u32) << 28) | (loc.offset + off as u32)),
    }
}

impl<'a> Loader<'a> {
    pub fn new(r: Reader<'a>) -> Self {
        Loader { r, assets: Vec::new(), script_strings: Vec::new(), texture_tables: HashMap::new() }
    }

    pub fn supports(ty: AssetType) -> bool {
        super::generic::struct_for_asset_type(ty).is_some()
    }

    fn load_generic(&mut self, ty: AssetType) -> Result<Asset> {
        let Some(st) = super::generic::struct_for_asset_type(ty) else {
            bail!("asset type {ty:?} has no schema");
        };
        let root = self.generic_asset(st)?;
        let name = ["name", "aliasName", "szInternalName", "fontName", "filename"]
            .iter()
            .find_map(|f| root.string(f).map(str::to_owned))
            .or_else(|| root.node("window").and_then(|w| w.string("name")).map(str::to_owned))
            .unwrap_or_default();
        Ok(Asset::Generic(GenericAsset { ty, name, root }))
    }

    fn hand_written(ty: AssetType) -> bool {
        use AssetType::*;
        matches!(
            ty,
            XModelPieces
                | PhysPreset
                | XModel
                | Material
                | TechniqueSet
                | Image
                | ClipMap
                | ClipMapPvs
                | ComWorld
                | GameWorldMp
                | MapEnts
                | GfxWorld
                | LightDef
                | RawFile
        )
    }

    /// Load an asset through a handle (`XModel*`, `Material*`, ...).
    /// `slot` is where the pointer field itself lives: once an asset has been
    /// loaded inline, later references to it point at that field.
    pub fn handle(&mut self, ptr: Ptr, ty: AssetType, slot: Option<u32>) -> Result<Option<AssetId>> {
        match ptr {
            Ptr::Null => Ok(None),
            Ptr::Ref { .. } => Ok(self.r.alias(ptr)),
            Ptr::Inline | Ptr::Insert => {
                let inserted = (ptr == Ptr::Insert).then(|| self.r.insert_alias_slot());
                let asset = if Self::hand_written(ty) {
                    self.r.push(BLOCK_TEMP);
                    let a = self.load_asset(ty);
                    self.r.pop();
                    a
                } else {
                    self.load_generic(ty)
                };
                let id = self.assets.len();
                self.assets.push(asset?);
                for key in [inserted, slot].into_iter().flatten() {
                    self.r.aliases.insert(key, id);
                }
                Ok(Some(id))
            }
        }
    }

    fn load_asset(&mut self, ty: AssetType) -> Result<Asset> {
        use AssetType as T;
        Ok(match ty {
            T::TechniqueSet => Asset::TechniqueSet(self.technique_set()?),
            T::Material => Asset::Material(self.material()?),
            T::Image => Asset::Image(self.image()?),
            T::XModel => Asset::XModel(self.xmodel()?),
            T::XModelPieces => Asset::XModelPieces(self.xmodel_pieces()?),
            T::PhysPreset => Asset::PhysPreset(self.phys_preset()?),
            T::LightDef => Asset::LightDef(self.light_def()?),
            T::ComWorld => Asset::ComWorld(self.com_world()?),
            T::GameWorldMp => Asset::GameWorldMp(self.game_world_mp()?),
            T::MapEnts => Asset::MapEnts(self.map_ents()?),
            T::GfxWorld => Asset::GfxWorld(Box::new(self.gfx_world()?)),
            T::ClipMap | T::ClipMapPvs => Asset::ClipMap(Box::new(self.clip_map()?)),
            T::RawFile => Asset::RawFile(self.raw_file()?),
            other => bail!("asset type {other:?} is not supported yet"),
        })
    }

    /// Load the struct image of an asset header into the temp block and
    /// switch to the virtual block for its members.
    fn begin(&mut self, size: usize) -> Result<S<'a>> {
        let (_, s) = self.r.array(4, size, 1)?;
        self.r.push(BLOCK_VIRTUAL);
        Ok(s)
    }

    fn end(&mut self) {
        self.r.pop();
    }

    // ------------------------------------------------------------ techniques

    fn technique_set(&mut self) -> Result<TechniqueSet> {
        let s = self.begin(148)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        // remappedTechniqueSet (@8) is never serialised.
        let mut techniques = Vec::with_capacity(34);
        for i in 0..34 {
            let p = s.ptr(12 + i * 4);
            techniques.push(if p.is_inline() { Some(self.technique()?) } else { None });
        }
        self.end();
        Ok(TechniqueSet { name, world_vert_format: s.u8(4), techniques })
    }

    fn technique(&mut self) -> Result<Technique> {
        // MaterialTechnique { name; u16 flags; u16 passCount; MaterialPass passArray[passCount]; }
        self.r.alloc(4);
        let head = S(self.r.read(8)?);
        let pass_count = head.u16(6) as usize;
        let passes = S(self.r.read(20 * pass_count)?);
        let mut out = Vec::with_capacity(pass_count);
        for i in 0..pass_count {
            let p = passes.elem(i, 20);
            let mut pass = TechniquePass::default();
            if p.ptr(0).is_inline() {
                self.r.array(4, 100, 1)?; // MaterialVertexDeclaration
            }
            if p.ptr(4).is_inline() {
                pass.vertex_shader = self.shader()?;
            }
            if p.ptr(8).is_inline() {
                pass.pixel_shader = self.shader()?;
            }
            let arg_count = p.u8(12) as usize + p.u8(13) as usize + p.u8(14) as usize;
            if p.ptr(16).is_inline() {
                let (_, args) = self.r.array(4, 8, arg_count)?;
                for a in 0..arg_count {
                    let arg = args.elem(a, 8);
                    let ty = arg.u16(0);
                    pass.args.push((ty, arg.u16(2), arg.u32(4)));
                    // MTL_ARG_LITERAL_VERTEX_CONST / MTL_ARG_LITERAL_PIXEL_CONST
                    if (ty == 1 || ty == 7) && arg.ptr(4).is_inline() {
                        self.r.array(4, 16, 1)?;
                    }
                }
            }
            out.push(pass);
        }
        let name = self.r.xstring_or_empty(head.ptr(0))?;
        Ok(Technique { name, passes: out })
    }

    /// MaterialVertexShader / MaterialPixelShader { name; { void* dx; { u32* program; u16 programSize; u16 } } }
    fn shader(&mut self) -> Result<Option<ShaderProgram>> {
        let (_, s) = self.r.array(4, 16, 1)?;
        let name = self.r.xstring(s.ptr(0))?.unwrap_or_default();
        if s.ptr(8).is_inline() {
            let (_, prog) = self.r.array(4, 4, s.u16(12) as usize)?;
            return Ok(Some(ShaderProgram { name, program: prog.0.to_vec() }));
        }
        Ok(None)
    }

    // ------------------------------------------------------------ materials

    fn material(&mut self) -> Result<Material> {
        let s = self.begin(80)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let technique_set = self.handle(s.ptr(64), AssetType::TechniqueSet, None)?;

        let texture_count = s.u8(58) as usize;
        let constant_count = s.u8(59) as usize;
        let state_bits_count = s.u8(60) as usize;

        let tex_ptr = s.ptr(68);
        let raw_textures = if tex_ptr.is_inline() {
            let (loc, t) = self.r.array(4, 12, texture_count)?;
            let mut out = Vec::new();
            for i in 0..texture_count {
                let e = t.elem(i, 12);
                let semantic = e.u8(7);
                let image = if semantic == 11 {
                    if e.ptr(8).is_inline() { self.water()? } else { None }
                } else {
                    self.handle(e.ptr(8), AssetType::Image, slot(loc, i * 12 + 8))?
                };
                out.push((e.u32(0), e.u8(4), e.u8(5), e.u8(6), semantic, image));
            }
            self.texture_tables.insert((loc.block, loc.offset), out.clone());
            out
        } else if let Ptr::Ref { block, offset } = tex_ptr {
            self.texture_tables.get(&(block, offset)).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        let textures = raw_textures
            .into_iter()
            .map(|(name_hash, name_start, name_end, sampler_state, semantic, image)| MaterialTexture {
                name_hash,
                name_start,
                name_end,
                sampler_state,
                semantic: TextureSemantic::from_u8(semantic),
                image,
            })
            .collect();

        let constants = match self.fixed_array(s.ptr(72), 16, 32, constant_count)? {
            Some(c) => (0..constant_count)
                .map(|i| {
                    let e = c.elem(i, 32);
                    MaterialConstant { name_hash: e.u32(0), name: e.cstr(4, 12), literal: e.vec4(16) }
                })
                .collect(),
            None => Vec::new(),
        };
        let state_bits = match self.fixed_array(s.ptr(76), 4, 8, state_bits_count)? {
            Some(c) => (0..state_bits_count)
                .map(|i| {
                    let e = c.elem(i, 8);
                    [e.u32(0), e.u32(4)]
                })
                .collect(),
            None => Vec::new(),
        };
        self.end();

        let mut state_bits_entry = [0u8; 34];
        state_bits_entry.copy_from_slice(&s.0[24..58]);
        Ok(Material {
            name,
            game_flags: s.u8(4),
            sort_key: s.u8(5),
            surface_type_bits: s.u32(16),
            state_bits_entry,
            state_flags: s.u8(61),
            camera_region: s.u8(62),
            technique_set,
            textures,
            constants,
            state_bits,
        })
    }

    /// A pointer-free array that may be inline or shared by reference.
    fn fixed_array(&mut self, ptr: Ptr, align: u32, size: usize, count: usize) -> Result<Option<S<'a>>> {
        Ok(match ptr {
            Ptr::Null => None,
            Ptr::Inline | Ptr::Insert => Some(self.r.array(align, size, count)?.1),
            Ptr::Ref { .. } => self.r.deref(ptr).map(|fp| S(&self.r.data()[fp..fp + size * count])),
        })
    }

    /// water_t: returns its image handle.
    fn water(&mut self) -> Result<Option<AssetId>> {
        let (wloc, w) = self.r.array(4, 68, 1)?;
        let n = (w.i32(12).max(0) * w.i32(16).max(0)) as usize;
        if w.ptr(4).is_inline() {
            self.r.array(4, 8, n)?;
        }
        if w.ptr(8).is_inline() {
            self.r.array(4, 4, n)?;
        }
        self.handle(w.ptr(64), AssetType::Image, slot(wloc, 64))
    }

    fn image(&mut self) -> Result<Image> {
        let s = self.begin(36)?;
        let name = self.r.xstring_or_empty(s.ptr(32))?;
        // loadDef lives in the temp block; a `-2` pointer still reserves an
        // alias slot in the insert (virtual) block first.
        if s.ptr(4) == Ptr::Insert {
            self.r.insert_alias_slot();
        }
        let load_def = if s.ptr(4).is_inline() {
            self.r.push(BLOCK_TEMP);
            self.r.alloc(4);
            let h = S(self.r.read(16)?);
            let data = self.r.read(h.u32(12) as usize)?.to_vec();
            self.r.pop();
            Some(ImageLoadDef {
                level_count: h.u8(0),
                flags: h.u8(1),
                dimensions: [h.u16(2), h.u16(4), h.u16(6)],
                format: h.u32(8),
                data,
            })
        } else {
            None
        };
        self.end();
        let map_type = match s.u32(0) {
            3 => MapType::TwoD,
            4 => MapType::ThreeD,
            5 => MapType::Cube,
            o => MapType::Other(o),
        };
        Ok(Image {
            name,
            map_type,
            semantic: s.u8(11),
            category: s.u8(30),
            width: s.u16(24),
            height: s.u16(26),
            depth: s.u16(28),
            load_def,
        })
    }

    // ------------------------------------------------------------ models

    fn xmodel(&mut self) -> Result<XModel> {
        let s = self.begin(220)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let num_bones = s.u8(4) as usize;
        let num_root = s.u8(5) as usize;
        let num_surfs = s.u8(6) as usize;
        let non_root = num_bones.saturating_sub(num_root);

        let bone_names = match self.fixed_array(s.ptr(8), 2, 2, num_bones)? {
            Some(a) => (0..num_bones)
                .map(|i| self.script_strings.get(a.u16(i * 2) as usize).cloned().unwrap_or_default())
                .collect(),
            None => Vec::new(),
        };
        let parent_list = self.fixed_array(s.ptr(12), 1, 1, non_root)?.map(|a| a.0.to_vec()).unwrap_or_default();
        let quats = self
            .fixed_array(s.ptr(16), 2, 8, non_root)?
            .map(|a| {
                (0..non_root).map(|i| [a.i16(i * 8), a.i16(i * 8 + 2), a.i16(i * 8 + 4), a.i16(i * 8 + 6)]).collect()
            })
            .unwrap_or_default();
        // Packed vec3s, but the allocation reserves 16 bytes per bone.
        let trans = self
            .fixed_array(s.ptr(20), 4, 16, non_root)?
            .map(|a| (0..non_root).map(|i| a.vec3(i * 12)).collect())
            .unwrap_or_default();
        self.fixed_array(s.ptr(24), 1, 1, num_bones)?; // partClassification
        let base_mat = self
            .fixed_array(s.ptr(28), 4, 32, num_bones)?
            .map(|a| (0..num_bones).map(|i| BoneMat { quat: a.vec4(i * 32), trans: a.vec3(i * 32 + 16) }).collect())
            .unwrap_or_default();
        let mut surfs = Vec::new();
        if s.ptr(32).is_inline() {
            let (_, arr) = self.r.array(4, 56, num_surfs)?;
            for i in 0..num_surfs {
                surfs.push(self.xsurface(arr.elem(i, 56))?);
            }
        }
        let mut materials = Vec::new();
        if s.ptr(36).is_inline() {
            let (loc, arr) = self.r.array(4, 4, num_surfs)?;
            for i in 0..num_surfs {
                materials.push(self.handle(arr.ptr(i * 4), AssetType::Material, slot(loc, i * 4))?);
            }
        }

        let lods = (0..4)
            .map(|i| {
                let o = 40 + i * 28;
                XModelLod { dist: s.f32(o), num_surfs: s.u16(o + 4), surf_index: s.u16(o + 6) }
            })
            .take(s.u16(196) as usize)
            .collect();

        let num_coll_surfs = s.i32(156).max(0) as usize;
        if s.ptr(152).is_inline() {
            let (_, arr) = self.r.array(4, 44, num_coll_surfs)?;
            for i in 0..num_coll_surfs {
                let c = arr.elem(i, 44);
                if c.ptr(0).is_inline() {
                    self.r.array(4, 48, c.i32(4).max(0) as usize)?;
                }
            }
        }
        self.fixed_array(s.ptr(164), 4, 40, num_bones)?; // boneInfo
        self.handle(s.ptr(212), AssetType::PhysPreset, None)?;
        if s.ptr(216).is_inline() {
            self.phys_geom_list()?;
        }
        self.end();

        Ok(XModel {
            name,
            num_bones: num_bones as u8,
            num_root_bones: num_root as u8,
            bone_names,
            parent_list,
            quats,
            trans,
            base_mat,
            surfs,
            materials,
            lods,
            radius: s.f32(168),
            mins: s.vec3(172),
            maxs: s.vec3(184),
            contents: s.i32(160),
        })
    }

    fn xsurface(&mut self, s: S<'a>) -> Result<XSurface> {
        let vert_count = s.u16(2) as usize;
        let tri_count = s.u16(4) as usize;
        let blend_counts = [s.i16(16), s.i16(18), s.i16(20), s.i16(22)];

        // Serialisation order: vertInfo, verts0, vertList, triIndices.
        let blend_len = blend_counts[0].max(0) as usize
            + 3 * blend_counts[1].max(0) as usize
            + 5 * blend_counts[2].max(0) as usize
            + 7 * blend_counts[3].max(0) as usize;
        let verts_blend = self
            .fixed_array(s.ptr(24), 2, 2, blend_len)?
            .map(|a| (0..blend_len).map(|i| a.u16(i * 2)).collect())
            .unwrap_or_default();

        let verts_ptr = s.ptr(28);
        let verts_data = match verts_ptr {
            Ptr::Inline | Ptr::Insert => Some(self.r.array_in(BLOCK_VERTEX, 16, 32, vert_count)?.1),
            Ptr::Ref { .. } => self.r.deref(verts_ptr).map(|fp| S(&self.r.data()[fp..fp + 32 * vert_count])),
            Ptr::Null => None,
        };
        let verts = verts_data
            .map(|a| {
                (0..vert_count)
                    .map(|i| {
                        let v = a.elem(i, 32);
                        PackedVertex {
                            xyz: v.vec3(0),
                            binormal_sign: v.f32(12),
                            color: v.u32(16),
                            tex_coord: v.u32(20),
                            normal: v.u32(24),
                            tangent: v.u32(28),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let list_count = s.u32(32) as usize;
        let mut vert_lists = Vec::new();
        if let Some(a) = self.fixed_list(s.ptr(36), list_count)? {
            for i in 0..list_count {
                let e = a.elem(i, 12);
                vert_lists.push(RigidVertList {
                    bone_offset: e.u16(0),
                    vert_count: e.u16(2),
                    tri_offset: e.u16(4),
                    tri_count: e.u16(6),
                });
            }
        }

        let tris_ptr = s.ptr(12);
        let tris_data = match tris_ptr {
            Ptr::Inline | Ptr::Insert => Some(self.r.array_in(BLOCK_INDEX, 16, 6, tri_count)?.1),
            Ptr::Ref { .. } => self.r.deref(tris_ptr).map(|fp| S(&self.r.data()[fp..fp + 6 * tri_count])),
            Ptr::Null => None,
        };
        let tris = tris_data
            .map(|a| (0..tri_count).map(|i| [a.u16(i * 6), a.u16(i * 6 + 2), a.u16(i * 6 + 4)]).collect())
            .unwrap_or_default();

        Ok(XSurface {
            tile_mode: s.u8(0),
            deformed: s.u8(1) != 0,
            vert_count: vert_count as u16,
            tri_count: tri_count as u16,
            base_tri_index: s.u16(8),
            base_vert_index: s.u16(10),
            blend_counts,
            verts_blend,
            verts,
            tris,
            vert_lists,
        })
    }

    /// XRigidVertList array, including each list's collision tree.
    fn fixed_list(&mut self, ptr: Ptr, count: usize) -> Result<Option<S<'a>>> {
        match ptr {
            Ptr::Inline | Ptr::Insert => {
                let (_, a) = self.r.array(4, 12, count)?;
                for i in 0..count {
                    let e = a.elem(i, 12);
                    if e.ptr(8).is_inline() {
                        // XSurfaceCollisionTree
                        let (_, t) = self.r.array(4, 40, 1)?;
                        if t.ptr(28).is_inline() {
                            self.r.array(16, 16, t.u32(24) as usize)?;
                        }
                        if t.ptr(36).is_inline() {
                            self.r.array(2, 2, t.u32(32) as usize)?;
                        }
                    }
                }
                Ok(Some(a))
            }
            Ptr::Ref { .. } => Ok(self.r.deref(ptr).map(|fp| S(&self.r.data()[fp..fp + 12 * count]))),
            Ptr::Null => Ok(None),
        }
    }

    fn phys_geom_list(&mut self) -> Result<()> {
        let (_, l) = self.r.array(4, 44, 1)?;
        let count = l.u32(0) as usize;
        if l.ptr(4).is_inline() {
            let (_, geoms) = self.r.array(4, 68, count)?;
            for i in 0..count {
                if geoms.elem(i, 68).ptr(0).is_inline() {
                    self.brush_wrapper()?;
                }
            }
        }
        Ok(())
    }

    fn brush_wrapper(&mut self) -> Result<()> {
        let (_, b) = self.r.array(4, 80, 1)?;
        let num_sides = b.u32(28) as usize;
        if b.ptr(32).is_inline() {
            self.brush_sides(num_sides)?;
        }
        if b.ptr(48).is_inline() {
            self.r.array(1, 1, b.i32(72).max(0) as usize)?;
        }
        if b.ptr(76).is_inline() {
            self.r.array(4, 20, num_sides)?;
        }
        Ok(())
    }

    /// cbrushside_t array; each side's plane may be inline.
    fn brush_sides(&mut self, count: usize) -> Result<(Loc, S<'a>)> {
        let (loc, sides) = self.r.array(4, 12, count)?;
        for i in 0..count {
            if sides.elem(i, 12).ptr(0).is_inline() {
                self.r.array(4, 20, 1)?;
            }
        }
        Ok((loc, sides))
    }

    fn xmodel_pieces(&mut self) -> Result<XModelPieces> {
        let s = self.begin(12)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let n = s.i32(4).max(0) as usize;
        let mut pieces = Vec::new();
        if s.ptr(8).is_inline() {
            let (loc, a) = self.r.array(4, 16, n)?;
            for i in 0..n {
                let e = a.elem(i, 16);
                pieces.push((self.handle(e.ptr(0), AssetType::XModel, slot(loc, i * 16))?, e.vec3(4)));
            }
        }
        self.end();
        Ok(XModelPieces { name, pieces })
    }

    fn phys_preset(&mut self) -> Result<PhysPreset> {
        let s = self.begin(44)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let sound_prefix = self.r.xstring(s.ptr(28))?.unwrap_or_default();
        self.end();
        Ok(PhysPreset {
            name,
            sound_prefix,
            mass: s.f32(8),
            bounce: s.f32(12),
            friction: s.f32(16),
            bullet_force_scale: s.f32(20),
            explosive_force_scale: s.f32(24),
            pieces_spread_fraction: s.f32(32),
            pieces_upward_velocity: s.f32(36),
        })
    }

    fn light_def(&mut self) -> Result<LightDef> {
        let s = self.begin(16)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let attenuation = self.handle(s.ptr(4), AssetType::Image, None)?;
        self.end();
        Ok(LightDef { name, attenuation })
    }

    fn raw_file(&mut self) -> Result<RawFile> {
        let s = self.begin(12)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let len = s.i32(4).max(0) as usize;
        let data = if s.ptr(8).is_inline() {
            let (_, d) = self.r.array(1, 1, len + 1)?;
            d.0[..len].to_vec()
        } else {
            Vec::new()
        };
        self.end();
        Ok(RawFile { name, data })
    }

    // ------------------------------------------------------------ world

    fn com_world(&mut self) -> Result<ComWorld> {
        let s = self.begin(16)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let n = s.u32(8) as usize;
        let mut primary_lights = Vec::new();
        if s.ptr(12).is_inline() {
            let (_, a) = self.r.array(4, 68, n)?;
            for i in 0..n {
                let l = a.elem(i, 68);
                let def_name = self.r.xstring(l.ptr(64))?;
                primary_lights.push(PrimaryLight {
                    kind: l.u8(0),
                    color: l.vec3(4),
                    dir: l.vec3(16),
                    origin: l.vec3(28),
                    radius: l.f32(40),
                    cos_half_fov_outer: l.f32(44),
                    cos_half_fov_inner: l.f32(48),
                    def_name,
                });
            }
        }
        self.end();
        Ok(ComWorld { name, primary_lights })
    }

    fn game_world_mp(&mut self) -> Result<String> {
        let s = self.begin(4)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        self.end();
        Ok(name)
    }

    fn map_ents(&mut self) -> Result<MapEnts> {
        let s = self.begin(12)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let n = s.i32(8).max(0) as usize;
        let entity_string = if s.ptr(4).is_inline() {
            let (_, d) = self.r.array(1, 1, n)?;
            let end = d.0.iter().position(|&c| c == 0).unwrap_or(n);
            String::from_utf8_lossy(&d.0[..end]).into_owned()
        } else {
            String::new()
        };
        self.end();
        Ok(MapEnts { name, entity_string })
    }

    fn gfx_world(&mut self) -> Result<GfxWorld> {
        let s = self.begin(732)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;
        let base_name = self.r.xstring_or_empty(s.ptr(4))?;
        let plane_count = s.i32(8).max(0) as usize;
        let node_count = s.i32(12).max(0) as usize;
        let index_count = s.i32(16).max(0) as usize;
        let surface_count = s.i32(24).max(0) as usize;
        let cell_count = s.i32(240).max(0) as usize;
        let primary_light_count = s.u32(220) as usize;

        let indices = self
            .fixed_array(s.ptr(20), 2, 2, index_count)?
            .map(|a| (0..index_count).map(|i| a.u16(i * 2)).collect())
            .unwrap_or_default();
        let sky_n = s.i32(32).max(0) as usize;
        let sky_start_surfs = self
            .fixed_array(s.ptr(36), 4, 4, sky_n)?
            .map(|a| (0..sky_n).map(|i| a.i32(i * 4)).collect())
            .unwrap_or_default();
        let sky_image = self.handle(s.ptr(40), AssetType::Image, None)?;

        // sunLight: GfxLight with a GfxLightDef handle.
        if s.ptr(200).is_inline() {
            let (lloc, l) = self.r.array(4, 64, 1)?;
            self.handle(l.ptr(60), AssetType::LightDef, slot(lloc, 60))?;
        }

        let probe_count = s.u32(228) as usize;
        let mut reflection_probes = Vec::new();
        if s.ptr(232).is_inline() {
            let (loc, a) = self.r.array(4, 16, probe_count)?;
            for i in 0..probe_count {
                let e = a.elem(i, 16);
                let image = self.handle(e.ptr(12), AssetType::Image, slot(loc, i * 16 + 12))?;
                reflection_probes.push(ReflectionProbe { origin: [e.f32(0), e.f32(4), e.f32(8)], image });
            }
        }
        // reflectionProbeTextures (@236): runtime block.

        // dpvsPlanes
        if s.ptr(244).is_inline() {
            self.r.array(4, 20, plane_count)?;
        }
        if s.ptr(248).is_inline() {
            self.r.array(2, 2, node_count)?;
        }
        // sceneEntCellBits (@252): runtime.

        if s.ptr(260).is_inline() {
            let (_, cells) = self.r.array(4, 56, cell_count)?;
            for i in 0..cell_count {
                self.gfx_cell(cells.elem(i, 56))?;
            }
        }

        let lightmap_count = s.i32(264).max(0) as usize;
        let mut lightmaps = Vec::new();
        if s.ptr(268).is_inline() {
            let (loc, a) = self.r.array(4, 8, lightmap_count)?;
            for i in 0..lightmap_count {
                let e = a.elem(i, 8);
                let primary = self.handle(e.ptr(0), AssetType::Image, slot(loc, i * 8))?;
                let secondary = self.handle(e.ptr(4), AssetType::Image, slot(loc, i * 8 + 4))?;
                lightmaps.push(LightmapPair { primary, secondary });
            }
        }

        // lightGrid
        let light_grid = {
            let row_axis = s.u32(292) as usize;
            let mins = [s.u16(280), s.u16(282), s.u16(284)];
            let maxs = [s.u16(286), s.u16(288), s.u16(290)];
            let rows =
                if row_axis < 3 { (maxs[row_axis] as i64 - mins[row_axis] as i64 + 1).max(0) as usize } else { 0 };
            let mut grid = LightGrid { mins, maxs, row_axis: row_axis as u32, col_axis: s.u32(296), ..Default::default() };
            if s.ptr(300).is_inline() {
                let (_, a) = self.r.array(2, 2, rows)?;
                grid.row_data_start = (0..rows).map(|i| a.u16(i * 2)).collect();
            }
            if s.ptr(308).is_inline() {
                let n = s.u32(304) as usize;
                let (_, a) = self.r.array(1, 1, n)?;
                grid.raw_row_data = a.0[..n].to_vec();
            }
            if s.ptr(316).is_inline() {
                let n = s.u32(312) as usize;
                let (_, a) = self.r.array(4, 4, n)?;
                grid.entries = (0..n)
                    .map(|i| LightGridEntry {
                        colors_index: a.u16(i * 4),
                        primary_light_index: a.u8(i * 4 + 2),
                        needs_trace: a.u8(i * 4 + 3),
                    })
                    .collect();
            }
            if s.ptr(324).is_inline() {
                let n = s.u32(320) as usize;
                let (_, a) = self.r.array(4, 168, n)?;
                grid.colors = (0..n)
                    .map(|i| LightGridColors(std::array::from_fn(|d| std::array::from_fn(|c| a.u8(i * 168 + d * 3 + c)))))
                    .collect();
            }
            grid
        };
        // lightmapPrimaryTextures / lightmapSecondaryTextures: runtime.

        let model_count = s.i32(336).max(0) as usize;
        let models = match self.fixed_array(s.ptr(340), 4, 56, model_count)? {
            Some(a) => (0..model_count)
                .map(|i| {
                    let m = a.elem(i, 56);
                    GfxBrushModel {
                        bounds: [m.vec3(24), m.vec3(36)],
                        surface_count: m.u16(48),
                        start_surf_index: m.u16(50),
                    }
                })
                .collect(),
            None => Vec::new(),
        };

        // OAT `reorder: ... materialMemory vd vld`: vd and vld are visited right
        // after materialMemory, i.e. between `models` and `sun`.
        let mm_count = s.i32(372).max(0) as usize;
        if s.ptr(376).is_inline() {
            let (loc, a) = self.r.array(4, 8, mm_count)?;
            for i in 0..mm_count {
                self.handle(a.elem(i, 8).ptr(0), AssetType::Material, slot(loc, i * 8))?;
            }
        }
        let vertex_count = s.u32(48) as usize;
        let vertices = match self.fixed_array(s.ptr(52), 4, 44, vertex_count)? {
            Some(a) => (0..vertex_count)
                .map(|i| {
                    let v = a.elem(i, 44);
                    WorldVertex {
                        xyz: v.vec3(0),
                        binormal_sign: v.f32(12),
                        color: v.u32(16),
                        tex_coord: [v.f32(20), v.f32(24)],
                        lmap_coord: [v.f32(28), v.f32(32)],
                        normal: v.u32(36),
                        tangent: v.u32(40),
                    }
                })
                .collect(),
            None => Vec::new(),
        };
        if s.ptr(64).is_inline() {
            self.r.array(1, 1, s.u32(60) as usize)?;
        }

        // sunflare_t @380.
        let sun_flare = SunFlare {
            valid: s.u8(380) != 0,
            sprite: self.handle(s.ptr(384), AssetType::Material, None)?,
            flare: self.handle(s.ptr(388), AssetType::Material, None)?,
            sprite_size: s.f32(392),
            flare_min_size: s.f32(396),
            flare_min_dot: s.f32(400),
            flare_max_size: s.f32(404),
            flare_max_dot: s.f32(408),
            flare_max_alpha: s.f32(412),
            flare_fade_in: s.i32(416),
            flare_fade_out: s.i32(420),
            blind_min_dot: s.f32(424),
            blind_max_dot: s.f32(428),
            blind_max_darken: s.f32(432),
            blind_fade_in: s.i32(436),
            blind_fade_out: s.i32(440),
            glare_min_dot: s.f32(444),
            glare_max_dot: s.f32(448),
            glare_max_lighten: s.f32(452),
            glare_fade_in: s.i32(456),
            glare_fade_out: s.i32(460),
            fx_position: s.vec3(464),
        };
        self.handle(s.ptr(540), AssetType::Image, None)?; // outdoorImage
        // cellCasterBits .. nonSunPrimaryLightForModelDynEnt: runtime.

        if s.ptr(572).is_inline() {
            let (_, a) = self.r.array(4, 12, primary_light_count)?;
            for i in 0..primary_light_count {
                let g = a.elem(i, 12);
                if g.ptr(4).is_inline() {
                    self.r.array(2, 2, g.u16(0) as usize)?;
                }
                if g.ptr(8).is_inline() {
                    self.r.array(2, 2, g.u16(2) as usize)?;
                }
            }
        }
        if s.ptr(576).is_inline() {
            let (_, a) = self.r.array(4, 8, primary_light_count)?;
            for i in 0..primary_light_count {
                let region = a.elem(i, 8);
                if region.ptr(4).is_inline() {
                    let hull_count = region.u32(0) as usize;
                    let (_, hulls) = self.r.array(4, 80, hull_count)?;
                    for h in 0..hull_count {
                        let hull = hulls.elem(h, 80);
                        if hull.ptr(76).is_inline() {
                            self.r.array(4, 20, hull.u32(72) as usize)?;
                        }
                    }
                }
            }
        }

        // dpvs (GfxWorldDpvsStatic @580)
        let smodel_count = s.u32(580) as usize;
        let static_surface_count = s.u32(584) as usize;
        let static_surface_count_no_decal = s.u32(588) as usize;
        if s.ptr(652).is_inline() {
            self.r.array(2, 2, static_surface_count + static_surface_count_no_decal)?;
        }
        if s.ptr(656).is_inline() {
            self.r.array(4, 28, smodel_count)?;
        }
        let mut surfaces = Vec::new();
        if s.ptr(660).is_inline() {
            let (loc, a) = self.r.array(4, 48, surface_count)?;
            for i in 0..surface_count {
                let e = a.elem(i, 48);
                let material = self.handle(e.ptr(16), AssetType::Material, slot(loc, i * 48 + 16))?;
                surfaces.push(GfxSurface {
                    first_vertex: e.i32(4),
                    vertex_count: e.u16(8),
                    tri_count: e.u16(10),
                    base_index: e.i32(12),
                    material,
                    lightmap_index: e.u8(20),
                    reflection_probe_index: e.u8(21),
                    primary_light_index: e.u8(22),
                    flags: e.u8(23),
                    bounds: [e.vec3(24), e.vec3(36)],
                });
            }
        }
        if s.ptr(664).is_inline() {
            self.r.array(4, 32, s.i32(224).max(0) as usize)?;
        }
        let mut static_models = Vec::new();
        if s.ptr(668).is_inline() {
            let (loc, a) = self.r.array(4, 76, smodel_count)?;
            for i in 0..smodel_count {
                let e = a.elem(i, 76);
                let model = self.handle(e.ptr(56), AssetType::XModel, slot(loc, i * 76 + 56))?;
                static_models.push(StaticModel {
                    model,
                    origin: e.vec3(4),
                    axis: [e.vec3(16), e.vec3(28), e.vec3(40)],
                    scale: e.f32(52),
                    cull_dist: e.f32(0),
                    flags: e.u8(72),
                });
            }
        }
        // surfaceMaterials, surfaceCastsSunShadow, dpvsDyn: runtime.

        self.end();

        let sun = SunParse {
            name: s.cstr(72, 64),
            ambient_scale: s.f32(136),
            ambient_color: s.vec3(140),
            diffuse_fraction: s.f32(152),
            sun_light: s.f32(156),
            sun_color: s.vec3(160),
            diffuse_color: s.vec3(172),
            angles: s.vec3(188),
        };
        Ok(GfxWorld {
            name,
            base_name,
            indices,
            vertices,
            surfaces,
            static_models,
            sky_start_surfs,
            sky_image,
            reflection_probes,
            sun,
            sun_flare,
            sun_color_from_bsp: s.vec3(204),
            lightmaps,
            mins: s.vec3(344),
            maxs: s.vec3(356),
            lit_surfs: s.u32(592)..s.u32(596),
            decal_surfs: s.u32(600)..s.u32(604),
            emissive_surfs: s.u32(608)..s.u32(612),
            models,
            light_grid,
        })
    }

    fn gfx_cell(&mut self, c: S<'a>) -> Result<()> {
        // aabbTree
        let tree_count = c.i32(24).max(0) as usize;
        if c.ptr(28).is_inline() {
            let (_, trees) = self.r.array(4, 44, tree_count)?;
            for t in 0..tree_count {
                let tree = trees.elem(t, 44);
                if tree.ptr(36).is_inline() {
                    self.r.array(2, 2, tree.u16(34) as usize)?;
                }
            }
        }
        // portals
        let portal_count = c.i32(32).max(0) as usize;
        if c.ptr(36).is_inline() {
            let (_, portals) = self.r.array(4, 68, portal_count)?;
            for p in 0..portal_count {
                let portal = portals.elem(p, 68);
                if portal.ptr(32).is_inline() {
                    bail!("inline GfxCell inside a portal is not supported");
                }
                if portal.ptr(36).is_inline() {
                    self.r.array(4, 12, portal.u8(40) as usize)?;
                }
            }
        }
        if c.ptr(44).is_inline() {
            self.r.array(4, 4, c.i32(40).max(0) as usize)?;
        }
        if c.ptr(52).is_inline() {
            self.r.array(1, 1, c.u8(48) as usize)?;
        }
        Ok(())
    }

    fn clip_map(&mut self) -> Result<ClipMap> {
        let s = self.begin(284)?;
        let name = self.r.xstring_or_empty(s.ptr(0))?;

        let plane_count = s.i32(8).max(0) as usize;
        let planes_ptr = s.ptr(12);
        let (planes_loc, planes_data) = match planes_ptr {
            Ptr::Inline | Ptr::Insert => {
                let (l, d) = self.r.array(4, 20, plane_count)?;
                (Some(l), Some(d))
            }
            Ptr::Ref { .. } => (None, self.r.deref(planes_ptr).map(|fp| S(&self.r.data()[fp..fp + 20 * plane_count]))),
            Ptr::Null => (None, None),
        };
        let planes = planes_data
            .map(|a| {
                (0..plane_count)
                    .map(|i| {
                        let p = a.elem(i, 20);
                        Plane { normal: p.vec3(0), dist: p.f32(12) }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let planes_base = base_of(planes_ptr, planes_loc);

        let smodel_count = s.u32(16) as usize;
        let mut static_models = Vec::new();
        if s.ptr(20).is_inline() {
            let (loc, a) = self.r.array(4, 80, smodel_count)?;
            for i in 0..smodel_count {
                let e = a.elem(i, 80);
                let model = self.handle(e.ptr(4), AssetType::XModel, slot(loc, i * 80 + 4))?;
                static_models.push(ClipStaticModel {
                    model,
                    origin: e.vec3(8),
                    inv_scaled_axis: [e.vec3(20), e.vec3(32), e.vec3(44)],
                    absmin: e.vec3(56),
                    absmax: e.vec3(68),
                });
            }
        }

        let material_count = s.u32(24) as usize;
        let materials = self
            .fixed_array(s.ptr(28), 4, 72, material_count)?
            .map(|a| {
                (0..material_count)
                    .map(|i| {
                        let m = a.elem(i, 72);
                        ClipMaterial { name: m.cstr(0, 64), surface_flags: m.i32(64), content_flags: m.i32(68) }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let side_count = s.u32(32) as usize;
        let sides_ptr = s.ptr(36);
        let (sides_loc, sides) = if sides_ptr.is_inline() {
            let (l, d) = self.brush_sides(side_count)?;
            (Some(l), Some(d))
        } else {
            (None, None)
        };
        let sides_base = base_of(sides_ptr, sides_loc);

        if s.ptr(44).is_inline() {
            self.r.array(1, 1, s.u32(40) as usize)?; // brushEdges
        }
        let node_count = s.u32(48) as usize;
        if s.ptr(52).is_inline() {
            let (_, nodes) = self.r.array(4, 8, node_count)?;
            for i in 0..node_count {
                if nodes.elem(i, 8).ptr(0).is_inline() {
                    self.r.array(4, 20, 1)?;
                }
            }
        }
        // OAT `reorder: ... leafs leafbrushes leafbrushNodes`: visited in place of
        // `leafs`, i.e. right after `nodes`.
        if s.ptr(60).is_inline() {
            self.r.array(4, 44, s.u32(56) as usize)?;
        }
        if s.ptr(76).is_inline() {
            self.r.array(2, 2, s.u32(72) as usize)?;
        }
        let node_count = s.u32(64) as usize;
        let mut leaf_brush_nodes = Vec::new();
        if s.ptr(68).is_inline() {
            let (_, nodes) = self.r.array(4, 20, node_count)?;
            for i in 0..node_count {
                let n = nodes.elem(i, 20);
                let count = n.i16(2);
                let mut node = LeafBrushNode { leaf_brush_count: count, brushes: Vec::new(), child_offsets: [0; 2] };
                if count > 0 {
                    let brushes = match n.ptr(8) {
                        p if p.is_inline() => Some(self.r.array(2, 2, count as usize)?.1),
                        p => self.fixed_array(p, 2, 2, count as usize)?,
                    };
                    if let Some(b) = brushes {
                        node.brushes = (0..count as usize).map(|k| b.u16(k * 2)).collect();
                    }
                } else {
                    node.child_offsets = [n.u16(16), n.u16(18)];
                }
                leaf_brush_nodes.push(node);
            }
        }
        if s.ptr(84).is_inline() {
            self.r.array(4, 4, s.u32(80) as usize)?; // leafsurfaces
        }
        let vert_count = s.u32(88) as usize;
        let verts = self
            .fixed_array(s.ptr(92), 4, 12, vert_count)?
            .map(|a| (0..vert_count).map(|i| a.vec3(i * 12)).collect())
            .unwrap_or_default();
        let tri_count = s.i32(96).max(0) as usize;
        let tri_indices = self
            .fixed_array(s.ptr(100), 2, 2, 3 * tri_count)?
            .map(|a| (0..3 * tri_count).map(|i| a.u16(i * 2)).collect())
            .unwrap_or_default();
        if s.ptr(104).is_inline() {
            self.r.array(1, 1, (3 * tri_count).div_ceil(32) * 4)?;
        }
        if s.ptr(112).is_inline() {
            self.r.array(4, 28, s.i32(108).max(0) as usize)?; // borders
        }
        let partition_count = s.i32(116).max(0) as usize;
        let mut partitions = Vec::new();
        if s.ptr(120).is_inline() {
            let (_, a) = self.r.array(4, 12, partition_count)?;
            for i in 0..partition_count {
                let p = a.elem(i, 12);
                if p.ptr(8).is_inline() {
                    self.r.array(4, 28, 1)?;
                }
                partitions.push(CollisionPartition { tri_count: p.u8(0), first_tri: p.i32(4) });
            }
        }
        let aabb_count = s.i32(124).max(0) as usize;
        let aabb_trees = self
            .fixed_array(s.ptr(128), 4, 32, aabb_count)?
            .map(|a| {
                (0..aabb_count)
                    .map(|i| {
                        let t = a.elem(i, 32);
                        CollisionAabbTree {
                            origin: t.vec3(0),
                            half_size: t.vec3(12),
                            material_index: t.u16(24),
                            child_count: t.u16(26),
                            index: t.i32(28),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cmodel_count = s.u32(132) as usize;
        let cmodels = self
            .fixed_array(s.ptr(136), 4, 72, cmodel_count)?
            .map(|a| (0..cmodel_count).map(|i| read_cmodel(a.elem(i, 72))).collect())
            .unwrap_or_default();

        let brush_count = s.u16(140) as usize;
        let mut brushes = Vec::new();
        if s.ptr(144).is_inline() {
            let (_, a) = self.r.array(16, 80, brush_count)?;
            // Sides are stored in brush order, so the lowest side reference is
            // the start of the brushsides array. This is exact even if our own
            // block bookkeeping has drifted.
            let sides_base = (0..brush_count)
                .filter_map(|i| match a.elem(i, 80).ptr(32) {
                    Ptr::Ref { block, offset } => Some((block, offset)),
                    _ => None,
                })
                .min_by_key(|&(_, o)| o)
                .or(sides_base);
            for i in 0..brush_count {
                let b = a.elem(i, 80);
                let n = b.u32(28) as usize;
                let first_side = if b.ptr(32).is_inline() {
                    bail!("inline brush sides on a clipMap brush are not supported");
                } else if n > 0 {
                    index_in(sides_base, b.ptr(32), 12)
                } else {
                    Some(0)
                };
                if b.ptr(48).is_inline() {
                    self.r.array(1, 1, 1)?;
                }
                let mut side_planes = Vec::new();
                let mut side_materials = Vec::new();
                if let (Some(first), Some(sides)) = (first_side, sides) {
                    for k in 0..n {
                        let idx = first as usize + k;
                        if idx >= side_count {
                            bail!("brush {i} side {idx} out of range ({side_count})");
                        }
                        let side = sides.elem(idx, 12);
                        let Some(plane) = index_in(planes_base, side.ptr(0), 20) else {
                            bail!("brush {i}: side plane does not point into the plane array");
                        };
                        side_planes.push(plane);
                        side_materials.push(side.u32(4));
                    }
                }
                brushes.push(Brush {
                    mins: b.vec3(0),
                    maxs: b.vec3(16),
                    contents: b.i32(12),
                    side_planes,
                    side_materials,
                    axial_materials: [[b.i16(36), b.i16(38), b.i16(40)], [b.i16(42), b.i16(44), b.i16(46)]],
                });
            }
        }

        if s.ptr(156).is_inline() {
            self.r.array(1, 1, (s.i32(148).max(0) * s.i32(152).max(0)) as usize)?;
        }
        let map_ents = self.handle(s.ptr(164), AssetType::MapEnts, None)?;
        if s.ptr(168).is_inline() {
            let (_, b) = self.r.array(16, 80, 1)?;
            if b.ptr(32).is_inline() {
                self.brush_sides(1)?;
            }
            if b.ptr(48).is_inline() {
                self.r.array(1, 1, 1)?;
            }
        }

        let dyn_ent_counts = [s.u16(244), s.u16(246)];
        let mut dyn_ents = Vec::new();
        for (k, &count) in dyn_ent_counts.iter().enumerate() {
            if s.ptr(248 + k * 4).is_inline() {
                let (loc, defs) = self.r.array(4, 96, count as usize)?;
                for i in 0..count as usize {
                    // `DynEntityDef` (0x60): type, pose (quat, origin), xModel,
                    // brushModel, physicsBrushModel, destroyFx, destroyPieces,
                    // physPreset, health, mass (36 bytes), contents.
                    let d = defs.elem(i, 96);
                    let model = self.handle(d.ptr(32), AssetType::XModel, slot(loc, i * 96 + 32))?;
                    let destroy_fx = if d.ptr(40) != Ptr::Null {
                        self.handle(d.ptr(40), AssetType::Fx, slot(loc, i * 96 + 40))?
                    } else {
                        None
                    };
                    let destroy_pieces = self.handle(d.ptr(44), AssetType::XModelPieces, slot(loc, i * 96 + 44))?;
                    let phys_preset = self.handle(d.ptr(48), AssetType::PhysPreset, slot(loc, i * 96 + 48))?;
                    dyn_ents.push(DynEntDef {
                        kind: d.i32(0),
                        quat: d.vec4(4),
                        origin: d.vec3(20),
                        model,
                        brush_model: d.u16(36),
                        physics_brush_model: d.u16(38),
                        destroy_fx,
                        destroy_pieces,
                        phys_preset,
                        health: d.i32(52),
                        contents: d.i32(92),
                    });
                }
            }
        }
        // dynEnt pose/client/coll lists: runtime.

        self.end();

        Ok(ClipMap {
            name,
            planes,
            static_models,
            materials,
            brushes,
            verts,
            tri_indices,
            partitions,
            aabb_trees,
            cmodels,
            map_ents,
            dyn_ent_counts,
            dyn_ents,
            leaf_brush_nodes,
        })
    }
}

fn read_cmodel(m: S) -> CModel {
    CModel {
        mins: m.vec3(0),
        maxs: m.vec3(12),
        radius: m.f32(24),
        first_coll_aabb: m.u16(28),
        coll_aabb_count: m.u16(30),
        leaf_brush_node: m.i32(28 + 36),
    }
}
