//! Schema-driven asset loader, adapted from `iw3::zone::generic`. World at
//! War assets are all loaded this way.
//!
//! It mirrors what OpenAssetTools' generated code does: read a struct, then
//! visit its pointer members (in serialisation order) according to the rules
//! in `schema.json`, recursing into embedded structs and loaded arrays.

use super::Loader;
use super::reader::*;
use super::schema::{schema, Member, MemberRules, Resolved, Schema};
use super::{AssetId, AssetType};
use anyhow::{anyhow, bail, Result};

/// A loaded struct instance and everything hanging off it.
#[derive(Debug, Clone)]
pub struct GNode {
    pub ty: String,
    pub data: Vec<u8>,
    pub fields: Vec<(String, GVal)>,
}

#[derive(Debug, Clone)]
pub enum GVal {
    /// An embedded struct or a loaded array of structs.
    Nodes(Vec<GNode>),
    /// A loaded array of primitives.
    Bytes(Vec<u8>),
    Str(Option<String>),
    Strs(Vec<Option<String>>),
    Asset(Option<AssetId>),
    Assets(Vec<Option<AssetId>>),
}

impl GNode {
    pub fn field(&self, name: &str) -> Option<&GVal> {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    pub fn nodes(&self, name: &str) -> &[GNode] {
        match self.field(name) {
            Some(GVal::Nodes(n)) => n,
            _ => &[],
        }
    }

    pub fn node(&self, name: &str) -> Option<&GNode> {
        self.nodes(name).first()
    }

    pub fn bytes(&self, name: &str) -> &[u8] {
        match self.field(name) {
            Some(GVal::Bytes(b)) => b,
            _ => &[],
        }
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        match self.field(name) {
            Some(GVal::Str(s)) => s.as_deref(),
            _ => None,
        }
    }

    pub fn strings(&self, name: &str) -> Vec<Option<&str>> {
        match self.field(name) {
            Some(GVal::Strs(s)) => s.iter().map(|s| s.as_deref()).collect(),
            Some(GVal::Str(s)) => vec![s.as_deref()],
            _ => Vec::new(),
        }
    }

    pub fn asset(&self, name: &str) -> Option<AssetId> {
        match self.field(name) {
            Some(GVal::Asset(a)) => *a,
            _ => None,
        }
    }

    pub fn assets(&self, name: &str) -> Vec<Option<AssetId>> {
        match self.field(name) {
            Some(GVal::Assets(a)) => a.clone(),
            Some(GVal::Asset(a)) => vec![*a],
            _ => Vec::new(),
        }
    }

    /// Read a scalar field (`a::b[2]` paths allowed) as an integer.
    pub fn int(&self, path: &str) -> i64 {
        read_path(schema(), &self.ty, &self.data, path).map(|v| v.as_i64()).unwrap_or(0)
    }

    pub fn float(&self, path: &str) -> f32 {
        read_path(schema(), &self.ty, &self.data, path).map(|v| v.as_f32()).unwrap_or(0.0)
    }
}

#[derive(Clone, Copy, Debug)]
enum Val {
    I(i64),
    F(f32),
}

impl Val {
    fn as_i64(self) -> i64 {
        match self {
            Val::I(i) => i,
            Val::F(f) => f as i64,
        }
    }

    fn as_f32(self) -> f32 {
        match self {
            Val::I(i) => i as f32,
            Val::F(f) => f,
        }
    }
}

/// Find a member by name, looking through anonymous structs/unions.
fn find_member<'s>(s: &'s Schema, ty: &str, name: &str) -> Option<(&'s Member, u32)> {
    let t = s.types.get(ty)?;
    for m in &t.members {
        if m.name.as_deref() == Some(name) {
            return Some((m, m.offset));
        }
        if m.anon || m.name.is_none() {
            if let Some((inner, off)) = find_member(s, &s.resolve(&m.ty).base, name) {
                return Some((inner, m.offset + off));
            }
        }
    }
    None
}

fn read_prim(base: &str, bytes: &[u8]) -> Option<Val> {
    let b = |n: usize| bytes.get(..n);
    Some(match base {
        "char" | "int8_t" => Val::I(*b(1)?.first()? as i8 as i64),
        "uchar" | "uint8_t" | "bool" => Val::I(*b(1)?.first()? as i64),
        "short" | "int16_t" => Val::I(i16::from_le_bytes(b(2)?.try_into().ok()?) as i64),
        "ushort" | "uint16_t" => Val::I(u16::from_le_bytes(b(2)?.try_into().ok()?) as i64),
        "int" | "int32_t" | "long" => Val::I(i32::from_le_bytes(b(4)?.try_into().ok()?) as i64),
        "uint" | "uint32_t" => Val::I(u32::from_le_bytes(b(4)?.try_into().ok()?) as i64),
        "float" => Val::F(f32::from_le_bytes(b(4)?.try_into().ok()?)),
        _ => Val::I(match bytes.len() {
            1.. if Schema::prim_size(base).is_none() => {
                // Enums: little-endian signed integer of their size.
                let n = bytes.len().min(4);
                let mut v = [0u8; 4];
                v[..n].copy_from_slice(&bytes[..n]);
                i32::from_le_bytes(v) as i64
            }
            _ => 0,
        }),
    })
}

/// Read a scalar through a `a::b[1]::c` path within one struct image.
fn read_path(s: &Schema, ty: &str, data: &[u8], path: &str) -> Option<Val> {
    let mut ty = ty.to_owned();
    let mut off = 0u32;
    let mut last: Option<(&Member, Resolved, u32)> = None;
    for seg in path.split("::") {
        let (name, idx) = match seg.find('[') {
            Some(i) => (&seg[..i], seg[i + 1..seg.len() - 1].parse::<u32>().ok()?),
            None => (seg, 0),
        };
        let (m, moff) = find_member(s, &ty, name)?;
        let r = s.resolve(&m.ty);
        let elem = if m.ptr + r.ptr > 0 { 4 } else { s.size_align(&r.base).0 * r.arr.iter().product::<u32>().max(1) };
        off += moff + idx * elem;
        ty = r.base.clone();
        last = Some((m, r, off));
    }
    let (m, r, off) = last?;
    let bytes = data.get(off as usize..)?;
    if let Some(bits) = m.bits {
        let raw = read_prim(&r.base, bytes)?.as_i64() as u64;
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        return Some(Val::I(((raw >> m.bit_offset) & mask) as i64));
    }
    if m.ptr + r.ptr > 0 {
        return Some(Val::I(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as i64));
    }
    let size = s.size_align(&r.base).0 as usize;
    read_prim(&r.base, bytes.get(..size)?)
}

/// A struct being processed, for evaluating rule expressions.
struct Frame<'d> {
    ty: String,
    data: &'d [u8],
}

fn eval(expr: &str, frames: &[Frame], owner: usize) -> Result<i64> {
    let toks = lex(expr);
    let mut p = ExprParser { toks: &toks, i: 0, frames, owner };
    let v = p.or()?;
    if p.i != toks.len() {
        bail!("trailing tokens in expression {expr:?}");
    }
    Ok(v)
}

fn lex(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            // Identifiers may contain `::` and `[n]` segments.
            let start = i;
            while i < c.len() {
                if c[i].is_ascii_alphanumeric() || c[i] == '_' {
                    i += 1;
                } else if c[i] == ':' && c.get(i + 1) == Some(&':') {
                    i += 2;
                } else if c[i] == '[' && start != i && !c[start].is_ascii_digit() {
                    while i < c.len() && c[i] != ']' {
                        i += 1;
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(c[start..i].iter().collect());
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||", "<<", ">>"].contains(&two.as_str()) {
                out.push(two);
                i += 2;
            } else {
                out.push(ch.to_string());
                i += 1;
            }
        }
    }
    out
}

struct ExprParser<'t, 'f, 'd> {
    toks: &'t [String],
    i: usize,
    frames: &'f [Frame<'d>],
    owner: usize,
}

impl ExprParser<'_, '_, '_> {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.i).map(String::as_str)
    }

    fn binary(&mut self, ops: &[&str], next: fn(&mut Self) -> Result<i64>) -> Result<i64> {
        let mut v = next(self)?;
        while let Some(op) = self.peek().filter(|t| ops.contains(t)).map(str::to_owned) {
            self.i += 1;
            let r = next(self)?;
            v = match op.as_str() {
                "||" => ((v != 0) || (r != 0)) as i64,
                "&&" => ((v != 0) && (r != 0)) as i64,
                "|" => v | r,
                "^" => v ^ r,
                "&" => v & r,
                "==" => (v == r) as i64,
                "!=" => (v != r) as i64,
                "<" => (v < r) as i64,
                "<=" => (v <= r) as i64,
                ">" => (v > r) as i64,
                ">=" => (v >= r) as i64,
                "<<" => v << r,
                ">>" => v >> r,
                "+" => v + r,
                "-" => v - r,
                "*" => v * r,
                "/" => {
                    if r == 0 {
                        0
                    } else {
                        v / r
                    }
                }
                "%" => {
                    if r == 0 {
                        0
                    } else {
                        v % r
                    }
                }
                _ => unreachable!(),
            };
        }
        Ok(v)
    }

    fn or(&mut self) -> Result<i64> {
        self.binary(&["||"], Self::and)
    }
    fn and(&mut self) -> Result<i64> {
        self.binary(&["&&"], Self::bitor)
    }
    fn bitor(&mut self) -> Result<i64> {
        self.binary(&["|"], Self::bitxor)
    }
    fn bitxor(&mut self) -> Result<i64> {
        self.binary(&["^"], Self::bitand)
    }
    fn bitand(&mut self) -> Result<i64> {
        self.binary(&["&"], Self::eq)
    }
    fn eq(&mut self) -> Result<i64> {
        self.binary(&["==", "!="], Self::rel)
    }
    fn rel(&mut self) -> Result<i64> {
        self.binary(&["<", "<=", ">", ">="], Self::shift)
    }
    fn shift(&mut self) -> Result<i64> {
        self.binary(&["<<", ">>"], Self::add)
    }
    fn add(&mut self) -> Result<i64> {
        self.binary(&["+", "-"], Self::mul)
    }
    fn mul(&mut self) -> Result<i64> {
        self.binary(&["*", "/", "%"], Self::unary)
    }

    fn unary(&mut self) -> Result<i64> {
        match self.peek() {
            Some("-") => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some("!") => {
                self.i += 1;
                Ok((self.unary()? == 0) as i64)
            }
            Some("~") => {
                self.i += 1;
                Ok(!self.unary()?)
            }
            Some("(") => {
                self.i += 1;
                let v = self.or()?;
                if self.peek() != Some(")") {
                    bail!("missing )");
                }
                self.i += 1;
                Ok(v)
            }
            Some(t) => {
                let t = t.to_owned();
                self.i += 1;
                self.atom(&t)
            }
            None => bail!("unexpected end of expression"),
        }
    }

    fn atom(&self, t: &str) -> Result<i64> {
        if let Some(hex) = t.strip_prefix("0x") {
            return Ok(i64::from_str_radix(hex, 16)?);
        }
        if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Ok(t.trim_end_matches('u').parse()?);
        }
        let s = schema();
        if let Some(v) = s.enums.get(t) {
            return Ok(*v);
        }
        // `maxs[rowAxis]`: work out non-numeric indices first.
        if let Some(open) = t.find('[') {
            let close = open + t[open..].find(']').ok_or_else(|| anyhow!("unclosed [ in {t:?}"))?;
            let inner = &t[open + 1..close];
            if !inner.chars().all(|c| c.is_ascii_digit()) {
                let index = self.atom(inner)?;
                return self.atom(&format!("{}[{index}]{}", &t[..open], &t[close + 1..]));
            }
        }
        // `Type::path` names an enclosing struct; otherwise try the rule's
        // owner frame first, then everything else from the inside out.
        if let Some((head, rest)) = t.split_once("::") {
            if let Some(f) = self.frames.iter().rev().find(|f| f.ty == head) {
                if let Some(v) = read_path(s, &f.ty, f.data, rest) {
                    return Ok(v.as_i64());
                }
            }
        }
        let order = std::iter::once(self.owner).chain((0..self.frames.len()).rev());
        for i in order {
            let f = &self.frames[i];
            if let Some(v) = read_path(s, &f.ty, f.data, t) {
                return Ok(v.as_i64());
            }
        }
        Err(anyhow!("cannot resolve {t:?} in {:?}", self.frames.iter().map(|f| &f.ty).collect::<Vec<_>>()))
    }
}

fn block_index(name: &str) -> usize {
    match name {
        "XFILE_BLOCK_TEMP" => BLOCK_TEMP,
        "XFILE_BLOCK_RUNTIME" => BLOCK_RUNTIME,
        "XFILE_BLOCK_LARGE_RUNTIME" => BLOCK_LARGE_RUNTIME,
        "XFILE_BLOCK_PHYSICAL_RUNTIME" => BLOCK_PHYSICAL_RUNTIME,
        "XFILE_BLOCK_LARGE" => BLOCK_LARGE,
        "XFILE_BLOCK_PHYSICAL" => BLOCK_PHYSICAL,
        _ => BLOCK_VIRTUAL,
    }
}

/// Map an OAT asset name to our asset type.
pub fn asset_type_for_struct(s: &Schema, base: &str) -> Option<AssetType> {
    use AssetType as T;
    Some(match s.assets.get(base)?.as_str() {
        "AssetPhysPreset" => T::PhysPreset,
        "AssetPhysConstraints" => T::PhysConstraints,
        "AssetDestructibleDef" => T::DestructibleDef,
        "AssetXAnim" => T::XAnimParts,
        "AssetXModel" => T::XModel,
        "AssetMaterial" => T::Material,
        "AssetTechniqueSet" => T::TechniqueSet,
        "AssetImage" => T::Image,
        "AssetSound" => T::Sound,
        "AssetLoadedSound" => T::LoadedSound,
        "AssetClipMapPvs" => T::ClipMapPvs,
        "AssetComWorld" => T::ComWorld,
        "AssetGameWorldSp" => T::GameWorldSp,
        "AssetGameWorldMp" => T::GameWorldMp,
        "AssetMapEnts" => T::MapEnts,
        "AssetGfxWorld" => T::GfxWorld,
        "AssetLightDef" => T::LightDef,
        "AssetFont" => T::Font,
        "AssetMenuList" => T::MenuList,
        "AssetMenu" => T::Menu,
        "AssetLocalize" => T::LocalizeEntry,
        "AssetWeapon" => T::Weapon,
        "AssetSoundDriverGlobals" => T::SndDriverGlobals,
        "AssetFx" => T::Fx,
        "AssetImpactFx" => T::ImpactFx,
        "AssetRawFile" => T::RawFile,
        "AssetStringTable" => T::StringTable,
        "AssetPackIndex" => T::PackIndex,
        _ => return None,
    })
}

pub fn struct_for_asset_type(ty: AssetType) -> Option<&'static str> {
    use AssetType as T;
    Some(match ty {
        T::PhysPreset => "PhysPreset",
        T::PhysConstraints => "PhysConstraints",
        T::DestructibleDef => "DestructibleDef",
        T::XAnimParts => "XAnimParts",
        T::XModel => "XModel",
        T::Material => "Material",
        T::TechniqueSet => "MaterialTechniqueSet",
        T::Image => "GfxImage",
        T::Sound => "snd_alias_list_t",
        T::LoadedSound => "LoadedSound",
        T::ClipMap | T::ClipMapPvs => "clipMap_t",
        T::ComWorld => "ComWorld",
        T::GameWorldSp => "GameWorldSp",
        T::GameWorldMp => "GameWorldMp",
        T::MapEnts => "MapEnts",
        T::GfxWorld => "GfxWorld",
        T::LightDef => "GfxLightDef",
        T::Font => "Font_s",
        T::MenuList => "MenuList",
        T::Menu => "menuDef_t",
        T::LocalizeEntry => "LocalizeEntry",
        T::Weapon => "WeaponDef",
        T::SndDriverGlobals => "SndDriverGlobals",
        T::Fx => "FxEffectDef",
        T::ImpactFx => "FxImpactTable",
        T::RawFile => "RawFile",
        T::StringTable => "StringTable",
        T::PackIndex => "PackIndex",
        // Types the PC game never streams (OpenAssetTools skips them too).
        T::XModelPieces | T::UiMap | T::AiType | T::MpType | T::Character | T::XModelAlias => return None,
    })
}

/// Where rules for a (possibly nested) member can be found: the struct that
/// owns them and the member path prefix within it.
#[derive(Clone)]
struct Scope {
    ty: String,
    prefix: String,
    frame: usize,
}

fn member_rules(s: &Schema, scopes: &[Scope], name: &str) -> (MemberRules, usize) {
    for sc in scopes.iter().rev() {
        let key = if sc.prefix.is_empty() { name.to_owned() } else { format!("{}::{name}", sc.prefix) };
        if let Some(r) = s.rules.get(&sc.ty).and_then(|r| r.members.get(&key)) {
            return (r.clone(), sc.frame);
        }
    }
    (MemberRules::default(), scopes.last().map_or(0, |s| s.frame))
}

/// Member processing order with OAT `reorder:` applied: listed members are
/// visited consecutively at the position of the first one.
fn ordered_members<'s>(s: &'s Schema, ty: &str) -> Vec<&'s Member> {
    let t = &s.types[ty];
    let mut order: Vec<&Member> = t.members.iter().collect();
    if let Some(list) = s.rules.get(ty).and_then(|r| r.ty.reorder.as_ref()) {
        let listed: Vec<&str> = list.iter().map(String::as_str).filter(|n| *n != "...").collect();
        if let Some(first) = listed.first() {
            if let Some(pos) = order.iter().position(|m| m.name.as_deref() == Some(*first)) {
                let picked: Vec<&Member> =
                    listed.iter().filter_map(|n| t.members.iter().find(|m| m.name.as_deref() == Some(*n))).collect();
                order.retain(|m| !listed.contains(&m.name.as_deref().unwrap_or("")));
                let at = pos.min(order.len());
                // Members before `first` that were removed shift the position.
                let removed_before =
                    t.members[..t.members.iter().position(|m| m.name.as_deref() == Some(*first)).unwrap_or(0)]
                        .iter()
                        .filter(|m| listed.contains(&m.name.as_deref().unwrap_or("")))
                        .count();
                let at = at.saturating_sub(removed_before);
                for (k, m) in picked.into_iter().enumerate() {
                    order.insert(at + k, m);
                }
            }
        }
    }
    order
}

/// `T4_TRACE_STRUCTS=1`: log every struct the loader visits.
fn trace_structs() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("T4_TRACE_STRUCTS").is_some())
}

fn has_pointers(s: &Schema, ty: &str, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    let Some(t) = s.types.get(ty) else { return false };
    t.members.iter().any(|m| {
        let r = s.resolve(&m.ty);
        m.ptr + r.ptr > 0 || (s.is_record(&r.base) && has_pointers(s, &r.base, depth + 1))
    })
}

impl<'a> Loader<'a> {
    /// Load an asset with the generic loader. `ty` is its struct name.
    pub(super) fn generic_asset(&mut self, ty: &str) -> Result<GNode> {
        let s = schema();
        let rules = s.rules.get(ty);
        let temp = rules.and_then(|r| r.ty.block.as_deref()) == Some("XFILE_BLOCK_TEMP");
        if temp {
            self.r.push(BLOCK_TEMP);
        }
        let align = rules.and_then(|r| r.ty.allocalign).unwrap_or(s.types[ty].align.max(1));
        let (loc, data) = self.alloc_struct(ty, align, &[])?;
        if temp {
            self.r.push(BLOCK_VIRTUAL);
        }
        let node = self.process_struct(ty, data, if temp { None } else { Some(loc) }, &mut Vec::new(), &[]);
        if temp {
            self.r.pop();
            self.r.pop();
        }
        node
    }

    /// Allocate and read one struct. Structs ending in an `arraysize`
    /// member (possibly inside nested unions) are variable-sized.
    fn alloc_struct(&mut self, ty: &str, align: u32, frames: &[(String, Vec<u8>)]) -> Result<(Loc, Vec<u8>)> {
        let s = schema();
        let loc = self.r.alloc(align);
        let has_dynamic = s.rules.get(ty).is_some_and(|r| r.members.values().any(|m| m.arraysize.is_some()));
        if !has_dynamic {
            return Ok((loc, self.r.read(s.types[ty].size as usize)?.to_vec()));
        }
        let mut data = Vec::new();
        let end = self.dynamic_end(ty, ty, "", 0, &mut data, frames)?;
        self.fill_to(&mut data, end as usize)?;
        Ok((loc, data))
    }

    fn fill_to(&mut self, data: &mut Vec<u8>, len: usize) -> Result<()> {
        if data.len() < len {
            let more = self.r.read(len - data.len())?;
            data.extend_from_slice(more);
        }
        Ok(())
    }

    /// End offset of a (possibly dynamic) struct `ty` placed at `base` within
    /// the top-level struct `top`, reading bytes as needed to decide.
    fn dynamic_end(
        &mut self,
        top: &str,
        ty: &str,
        prefix: &str,
        base: u32,
        data: &mut Vec<u8>,
        frames: &[(String, Vec<u8>)],
    ) -> Result<u32> {
        let s = schema();
        let t = &s.types[ty];
        let rules = s.rules.get(top);
        let path = |name: &str| if prefix.is_empty() { name.to_owned() } else { format!("{prefix}::{name}") };
        let contains_dynamic = |p: &str| rules.is_some_and(|r| r.members.iter().any(|(k, m)| m.arraysize.is_some() && (k == p || k.starts_with(&format!("{p}::")))));
        let eval_here = |expr: &str, data: &[u8]| -> Result<i64> {
            let mut fr: Vec<Frame> = frames.iter().map(|(t, d)| Frame { ty: t.clone(), data: d }).collect();
            fr.push(Frame { ty: top.to_owned(), data });
            let owner = fr.len() - 1;
            eval(expr, &fr, owner)
        };
        let members: Vec<&Member> = if t.kind == "union" {
            // Pick the active member like the loader does.
            let mut chosen = None;
            let mut fallback = None;
            for m in &t.members {
                let p = path(m.name.as_deref().unwrap_or(""));
                match rules.and_then(|r| r.members.get(&p)).and_then(|r| r.condition.as_deref()) {
                    Some("never") => {}
                    Some(cond) => {
                        self.fill_to(data, base as usize)?;
                        if chosen.is_none() && eval_here(cond, data)? != 0 {
                            chosen = Some(m);
                        }
                    }
                    None => {
                        if fallback.is_none() {
                            fallback = Some(m);
                        }
                    }
                }
            }
            let active: Vec<&Member> = chosen.or(fallback).into_iter().collect();
            if let Some(m) = active.first().filter(|m| !contains_dynamic(&path(m.name.as_deref().unwrap_or("")))) {
                // A fixed-size member of a dynamic union: the struct ends there.
                return Ok(base + m.offset + m.size);
            }
            active
        } else {
            t.members.iter().collect()
        };
        for m in members {
            let p = path(m.name.as_deref().unwrap_or(""));
            if !contains_dynamic(&p) {
                continue;
            }
            let off = base + m.offset;
            if let Some(expr) = rules.and_then(|r| r.members.get(&p)).and_then(|r| r.arraysize.clone()) {
                self.fill_to(data, off as usize)?;
                let count = eval_here(&expr, data)?.max(0) as u32;
                let elem = s.size_align(&s.resolve(&m.ty).base).0;
                return Ok(off + count * elem);
            }
            return self.dynamic_end(top, &s.resolve(&m.ty).base, &p, off, data, frames);
        }
        Ok(base + if t.kind == "union" { 0 } else { t.size })
    }

    fn process_struct(
        &mut self,
        ty: &str,
        data: Vec<u8>,
        base: Option<Loc>,
        frames: &mut Vec<(String, Vec<u8>)>,
        outer: &[Scope],
    ) -> Result<GNode> {
        let s = schema();
        if trace_structs() {
            eprintln!("{:indent$}{ty} @ {:#x} {:?}", "", self.r.file_pos(), &data[..data.len().min(24)], indent = frames.len() * 2);
        }
        let mut node = GNode { ty: ty.to_owned(), data, fields: Vec::new() };
        if !has_pointers(s, ty, 0) {
            return Ok(node);
        }
        frames.push((ty.to_owned(), node.data.clone()));
        let frame_idx = frames.len() - 1;
        let mut scopes = outer.to_vec();
        scopes.push(Scope { ty: ty.to_owned(), prefix: String::new(), frame: frame_idx });
        let result = self.process_members(ty, &mut node, base, frames, &scopes);
        frames.pop();
        result.map(|_| node)
    }

    fn process_members(
        &mut self,
        ty: &str,
        node: &mut GNode,
        base: Option<Loc>,
        frames: &mut Vec<(String, Vec<u8>)>,
        scopes: &[Scope],
    ) -> Result<()> {
        let s = schema();
        let is_union = s.types[ty].kind == "union";
        let mut members = ordered_members(s, ty);
        if is_union {
            // A union loads one member: the first whose condition holds, or
            // failing that the first one without a condition.
            let mut chosen = None;
            let mut fallback = None;
            for m in &members {
                let (rules, owner) = member_rules(s, scopes, m.name.as_deref().unwrap_or(""));
                match rules.condition.as_deref().map(str::trim) {
                    Some("never") => {}
                    Some(cond) => {
                        let fr: Vec<Frame> = frames.iter().map(|(t, d)| Frame { ty: t.clone(), data: d }).collect();
                        if chosen.is_none() && eval(cond, &fr, owner)? != 0 {
                            chosen = Some(*m);
                        }
                    }
                    None => {
                        if fallback.is_none() {
                            fallback = Some(*m);
                        }
                    }
                }
            }
            members = chosen.or(fallback).into_iter().collect();
        }
        for m in members {
            let name = m.name.clone().unwrap_or_default();
            let r = s.resolve(&m.ty);
            let ptr = m.ptr + r.ptr;
            if ptr == 0 && !(s.is_record(&r.base) && has_pointers(s, &r.base, 0)) {
                continue;
            }
            let (rules, owner) = member_rules(s, scopes, &name);
            if !is_union {
                if let Some(cond) = &rules.condition {
                    if cond.trim() == "never" {
                        continue;
                    }
                    let fr: Vec<Frame> = frames.iter().map(|(t, d)| Frame { ty: t.clone(), data: d }).collect();
                    if eval(cond, &fr, owner)? == 0 {
                        continue;
                    }
                }
            }
            let field_base = base.map(|l| Loc { offset: l.offset + m.offset, ..l });
            let val = if ptr == 0 {
                // A trailing variable-length array (`passArray[1]` holding
                // `passCount` passes) has as many elements as its rule says.
                let count = match &rules.arraysize {
                    Some(e) => {
                        let fr: Vec<Frame> = frames.iter().map(|(t, d)| Frame { ty: t.clone(), data: d }).collect();
                        Some(eval(e, &fr, owner)?.max(0) as u32)
                    }
                    None => None,
                };
                self.embedded(m, &r, node, field_base, frames, scopes, &name, count)?
            } else {
                self.pointer_member(m, &r, &rules, owner, node, field_base, frames)?
            };
            if let Some(v) = val {
                node.fields.push((name, v));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn embedded(
        &mut self,
        m: &Member,
        r: &Resolved,
        node: &GNode,
        field_base: Option<Loc>,
        frames: &mut Vec<(String, Vec<u8>)>,
        scopes: &[Scope],
        name: &str,
        count: Option<u32>,
    ) -> Result<Option<GVal>> {
        let s = schema();
        let (size, _) = s.size_align(&r.base);
        let count: u32 = count.unwrap_or_else(|| m.arr.iter().chain(&r.arr).product::<u32>().max(1));
        let mut out = Vec::new();
        for i in 0..count {
            let off = (m.offset + i * size) as usize;
            // Variable-sized structs may end before the declared size.
            let mut slice = node.data.get(off.min(node.data.len())..(off + size as usize).min(node.data.len())).unwrap_or(&[]).to_vec();
            slice.resize(size as usize, 0);
            let slice = &slice[..];
            let mut sub_scopes: Vec<Scope> = scopes
                .iter()
                .map(|sc| Scope {
                    ty: sc.ty.clone(),
                    prefix: if sc.prefix.is_empty() { name.to_owned() } else { format!("{}::{name}", sc.prefix) },
                    frame: sc.frame,
                })
                .collect();
            if m.anon {
                sub_scopes = scopes.to_vec();
            }
            let b = field_base.map(|l| Loc { offset: l.offset + i * size, ..l });
            out.push(self.process_struct(&r.base, slice.to_vec(), b, frames, &sub_scopes)?);
        }
        Ok(Some(GVal::Nodes(out)))
    }

    #[allow(clippy::too_many_arguments)]
    fn pointer_member(
        &mut self,
        m: &Member,
        r: &Resolved,
        rules: &MemberRules,
        owner: usize,
        node: &GNode,
        field_base: Option<Loc>,
        frames: &mut Vec<(String, Vec<u8>)>,
    ) -> Result<Option<GVal>> {
        let s = schema();
        let ptr_level = m.ptr + r.ptr;
        // Arrays of pointers embedded in the struct (`T* x[N]`).
        let slots: u32 = m.arr.iter().product::<u32>().max(1);
        let mut vals = Vec::new();
        for i in 0..slots {
            let raw = S(&node.data).ptr((m.offset + i * 4) as usize);
            let slot_loc = field_base.map(|l| Loc { offset: l.offset + i * 4, ..l });
            let mut rules = rules.clone();
            if !m.arr.is_empty() {
                // Per-element rules like `set count dynEntDefList[1] ...`.
                let (r2, _) = {
                    let name = format!("{}[{i}]", m.name.as_deref().unwrap_or(""));
                    let sc = &frames[owner];
                    (s.rules.get(&sc.0).and_then(|x| x.members.get(&name)).cloned(), ())
                };
                if let Some(r2) = r2 {
                    if r2.count.is_some() {
                        rules.count = r2.count;
                    }
                }
            }
            vals.push(self.load_pointer(raw, ptr_level, r, m, &rules, owner, slot_loc, frames)?);
        }
        if m.arr.is_empty() {
            return Ok(vals.pop());
        }
        // Flatten arrays of pointers into one value.
        Ok(Some(match vals.first() {
            Some(GVal::Asset(_)) => GVal::Assets(
                vals.into_iter().map(|v| if let GVal::Asset(a) = v { a } else { None }).collect(),
            ),
            Some(GVal::Str(_)) => {
                GVal::Strs(vals.into_iter().map(|v| if let GVal::Str(a) = v { a } else { None }).collect())
            }
            _ => GVal::Nodes(vals.into_iter().flat_map(|v| if let GVal::Nodes(n) = v { n } else { Vec::new() }).collect()),
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn load_pointer(
        &mut self,
        raw: Ptr,
        ptr_level: u32,
        r: &Resolved,
        m: &Member,
        rules: &MemberRules,
        owner: usize,
        slot: Option<Loc>,
        frames: &mut Vec<(String, Vec<u8>)>,
    ) -> Result<GVal> {
        let s = schema();
        let slot_key = |l: Option<Loc>| l.filter(|l| l.block != BLOCK_TEMP && l.block < BLOCK_COUNT && !matches!(l.block, BLOCK_RUNTIME | BLOCK_LARGE_RUNTIME | BLOCK_PHYSICAL_RUNTIME)).map(|l| key(l.block, l.offset));
        let count = |frames: &Vec<(String, Vec<u8>)>| -> Result<usize> {
            Ok(match &rules.count {
                Some(e) => {
                    let fr: Vec<Frame> = frames.iter().map(|(t, d)| Frame { ty: t.clone(), data: d }).collect();
                    eval(e, &fr, owner)?.max(0) as usize
                }
                None => 1,
            })
        };

        // Strings.
        if rules.string && r.base == "char" {
            if ptr_level == 1 {
                return Ok(GVal::Str(self.r.xstring(raw)?));
            }
            if !raw.is_inline() {
                return Ok(GVal::Strs(Vec::new()));
            }
            let n = count(frames)?;
            let (_, arr) = self.r.array(4, 4, n)?;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                out.push(self.r.xstring(arr.ptr(i * 4))?);
            }
            return Ok(GVal::Strs(out));
        }

        // Asset handles: `T*` (single) or `T**` (array of handles).
        if let Some(at) = asset_type_for_struct(s, &r.base) {
            if ptr_level == 1 {
                return Ok(GVal::Asset(self.handle(raw, at, slot_key(slot))?));
            }
            if !raw.is_inline() {
                return Ok(GVal::Assets(Vec::new()));
            }
            let n = count(frames)?;
            let (loc, arr) = self.r.array(4, 4, n)?;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let k = slot_key(Some(Loc { offset: loc.offset + i as u32 * 4, ..loc }));
                out.push(self.handle(arr.ptr(i * 4), at, k)?);
            }
            return Ok(GVal::Assets(out));
        }

        // A reusable member pointing back at data loaded earlier (models
        // sharing a skeleton, head variants sharing a mesh).
        if let Ptr::Ref { block, offset } = raw {
            let earlier = rules.reusable.then(|| self.r.reused.get(&key(block, offset)).cloned()).flatten();
            return Ok(earlier.unwrap_or(GVal::Nodes(Vec::new())));
        }
        if !raw.is_inline() {
            return Ok(GVal::Nodes(Vec::new()));
        }
        if raw == Ptr::Insert {
            self.r.insert_alias_slot();
        }
        let block = rules.block.as_deref().map(block_index);
        if let Some(b) = block {
            self.r.push(b);
        }
        let result = (|| -> Result<GVal> {
            let n = count(frames)?;
            if ptr_level >= 2 {
                // Array of pointers to structs/primitives.
                let (_, arr) = self.r.array(4, 4, n)?;
                let mut out = Vec::new();
                let inner = Resolved { ptr: 0, ..r.clone() };
                for i in 0..n {
                    if let GVal::Nodes(mut v) = self.load_pointer(arr.ptr(i * 4), 1, &inner, m, &MemberRules { count: None, ..rules.clone() }, owner, None, frames)? {
                        out.append(&mut v);
                    }
                }
                return Ok(GVal::Nodes(out));
            }
            let (elem_size, elem_align) = {
                let (sz, al) = s.size_align(&r.base);
                let dims: u32 = r.arr.iter().chain(&m.pointee_arr).product::<u32>().max(1);
                (sz * dims, r.align.unwrap_or(al))
            };
            let align = rules.allocalign.as_deref().and_then(|a| a.parse().ok()).unwrap_or(elem_align).max(1);
            if self.r.in_runtime_block() {
                self.r.alloc(align);
                self.r.read(elem_size as usize * n)?;
                return Ok(GVal::Bytes(Vec::new()));
            }
            if s.is_record(&r.base) && n == 1 && s.rules.get(&r.base).is_some_and(|x| x.members.values().any(|mr| mr.arraysize.is_some())) {
                let (loc, data) = self.alloc_struct(&r.base, align, frames)?;
                let node = self.process_struct(&r.base, data, Some(loc), frames, &[])?;
                let v = GVal::Nodes(vec![node]);
                self.remember(rules, loc, &v);
                return Ok(v);
            }
            let (loc, arr) = self.r.array(align, elem_size as usize, n)?;
            let v = if s.is_record(&r.base) && has_pointers(s, &r.base, 0) && r.arr.is_empty() && m.pointee_arr.is_empty() {
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    let elem = arr.elem(i, elem_size as usize).0.to_vec();
                    let l = Loc { offset: loc.offset + i as u32 * elem_size, ..loc };
                    out.push(self.process_struct(&r.base, elem, Some(l), frames, &[])?);
                }
                GVal::Nodes(out)
            } else if s.is_record(&r.base) {
                GVal::Nodes(
                    (0..n)
                        .map(|i| GNode { ty: r.base.clone(), data: arr.elem(i, elem_size as usize).0.to_vec(), fields: Vec::new() })
                        .collect(),
                )
            } else {
                GVal::Bytes(arr.0.to_vec())
            };
            self.remember(rules, loc, &v);
            Ok(v)
        })();
        if block.is_some() {
            self.r.pop();
        }
        result
    }

    /// Keep a reusable member's data for later pointers to it.
    fn remember(&mut self, rules: &MemberRules, loc: Loc, v: &GVal) {
        let persistent = !matches!(loc.block, BLOCK_TEMP | BLOCK_RUNTIME | BLOCK_LARGE_RUNTIME | BLOCK_PHYSICAL_RUNTIME);
        if rules.reusable && persistent {
            self.r.reused.insert(key(loc.block, loc.offset), v.clone());
        }
    }
}
