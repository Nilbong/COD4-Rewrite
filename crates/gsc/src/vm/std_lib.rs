//! Builtins that need nothing of the game: maths, strings, arrays, structs.
//! (CoD4's angles are degrees.)

use super::{Vm, value::*};
use std::rc::Rc;

fn f(args: &[Value], i: usize) -> Result<f32, String> {
    args.get(i).and_then(Value::as_f32).ok_or_else(|| format!("argument {} isn't a number", i + 1))
}

fn v(args: &[Value], i: usize) -> Result<[f32; 3], String> {
    args.get(i).and_then(Value::as_vec).ok_or_else(|| format!("argument {} isn't a vector", i + 1))
}

fn s(args: &[Value], i: usize) -> Result<&str, String> {
    args.get(i).and_then(Value::as_str).ok_or_else(|| format!("argument {} isn't a string", i + 1))
}

fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// CoD's angles (pitch, yaw, roll) to its forward, right and up.
pub fn angle_vectors(a: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sp, cp) = a[0].to_radians().sin_cos();
    let (sy, cy) = a[1].to_radians().sin_cos();
    let (sr, cr) = a[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    (forward, right, up)
}

pub fn vector_to_angles(d: [f32; 3]) -> [f32; 3] {
    if d[0] == 0.0 && d[1] == 0.0 {
        return [if d[2] > 0.0 { 270.0 } else { 90.0 }, 0.0, 0.0];
    }
    let yaw = d[1].atan2(d[0]).to_degrees().rem_euclid(360.0);
    let pitch = (-d[2]).atan2((d[0] * d[0] + d[1] * d[1]).sqrt()).to_degrees().rem_euclid(360.0);
    [pitch, yaw, 0.0]
}

/// A builtin of this kind by name, or `None` for the game's.
pub fn call(vm: &mut Vm, name: &str, _this: Option<&Value>, args: &[Value]) -> Option<Result<Value, String>> {
    Some((|| -> Result<Value, String> {
        Ok(match name {
            "isdefined" => Value::Int(i32::from(args.first().is_some_and(Value::is_defined))),
            "isstring" => Value::Int(i32::from(matches!(args.first(), Some(Value::Str(_))))),
            "isarray" => Value::Int(i32::from(matches!(args.first(), Some(Value::Array(_))))),
            "int" => match args.first() {
                Some(Value::Str(x)) => Value::Int(x.trim().parse::<f32>().map(|x| x as i32).unwrap_or(0)),
                _ => Value::Int(f(args, 0)? as i32),
            },
            "float" => match args.first() {
                Some(Value::Str(x)) => Value::Float(x.trim().parse().unwrap_or(0.0)),
                _ => Value::Float(f(args, 0)?),
            },
            "abs" => match args.first() {
                Some(Value::Int(i)) => Value::Int(i.abs()),
                _ => Value::Float(f(args, 0)?.abs()),
            },
            "min" => Value::Float(f(args, 0)?.min(f(args, 1)?)),
            "max" => Value::Float(f(args, 0)?.max(f(args, 1)?)),
            "floor" => Value::Float(f(args, 0)?.floor()),
            "ceil" => Value::Float(f(args, 0)?.ceil()),
            "sqrt" => Value::Float(f(args, 0)?.max(0.0).sqrt()),
            "squared" => Value::Float(f(args, 0)?.powi(2)),
            "sin" => Value::Float(f(args, 0)?.to_radians().sin()),
            "cos" => Value::Float(f(args, 0)?.to_radians().cos()),
            "tan" => Value::Float(f(args, 0)?.to_radians().tan()),
            "asin" => Value::Float(f(args, 0)?.clamp(-1.0, 1.0).asin().to_degrees()),
            "acos" => Value::Float(f(args, 0)?.clamp(-1.0, 1.0).acos().to_degrees()),
            "atan" => Value::Float(f(args, 0)?.atan().to_degrees()),
            "length" => Value::Float(len(v(args, 0)?)),
            "lengthsquared" => Value::Float(len(v(args, 0)?).powi(2)),
            "distance" => Value::Float(len(sub(v(args, 0)?, v(args, 1)?))),
            "distancesquared" => Value::Float(len(sub(v(args, 0)?, v(args, 1)?)).powi(2)),
            "distance2d" => {
                let d = sub(v(args, 0)?, v(args, 1)?);
                Value::Float((d[0] * d[0] + d[1] * d[1]).sqrt())
            }
            "vectornormalize" => {
                let a = v(args, 0)?;
                let l = len(a);
                Value::Vector(if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { a })
            }
            "vectordot" => {
                let (a, b) = (v(args, 0)?, v(args, 1)?);
                Value::Float(a[0] * b[0] + a[1] * b[1] + a[2] * b[2])
            }
            "vectorscale" => {
                let (a, k) = (v(args, 0)?, f(args, 1)?);
                Value::Vector([a[0] * k, a[1] * k, a[2] * k])
            }
            "vectortoangles" => Value::Vector(vector_to_angles(v(args, 0)?)),
            "anglestoforward" => Value::Vector(angle_vectors(v(args, 0)?).0),
            "anglestoright" => Value::Vector(angle_vectors(v(args, 0)?).1),
            "anglestoup" => Value::Vector(angle_vectors(v(args, 0)?).2),
            "combineangles" => {
                let (a, b) = (v(args, 0)?, v(args, 1)?);
                Value::Vector([a[0] + b[0], a[1] + b[1], a[2] + b[2]])
            }
            "randomint" => {
                let n = f(args, 0)? as i64;
                Value::Int(if n <= 0 { 0 } else { (vm.random() % n as u64) as i32 })
            }
            "randomintrange" => {
                let (lo, hi) = (f(args, 0)? as i64, f(args, 1)? as i64);
                Value::Int(if hi <= lo { lo as i32 } else { (lo + (vm.random() % (hi - lo) as u64) as i64) as i32 })
            }
            "randomfloat" => Value::Float((vm.random() % 1_000_000) as f32 / 1_000_000.0 * f(args, 0)?),
            "randomfloatrange" => {
                let (lo, hi) = (f(args, 0)?, f(args, 1)?);
                Value::Float(lo + (vm.random() % 1_000_000) as f32 / 1_000_000.0 * (hi - lo))
            }
            "gettime" => Value::Int((vm.time * 1000.0) as i32),
            "tolower" => Value::Str(s(args, 0)?.to_lowercase().into()),
            "toupper" => Value::Str(s(args, 0)?.to_uppercase().into()),
            "getsubstr" => {
                let text: Vec<char> = s(args, 0)?.chars().collect();
                let from = (f(args, 1)? as usize).min(text.len());
                let to = args.get(2).and_then(Value::as_f32).map_or(text.len(), |t| (t as usize).min(text.len())).max(from);
                Value::Str(text[from..to].iter().collect::<String>().into())
            }
            "issubstr" => Value::Int(i32::from(s(args, 0)?.contains(s(args, 1)?))),
            "strtok" => {
                let (text, seps) = (s(args, 0)?, s(args, 1)?);
                Value::array(text.split(|c| seps.contains(c)).filter(|t| !t.is_empty()).map(Value::str))
            }
            "spawnstruct" => vm.spawn_struct(),
            "getarraykeys" => match args.first() {
                Some(Value::Array(a)) => Value::array(a.keys().map(Key::value)),
                _ => Value::Array(Rc::new(Array::default())),
            },
            "assert" | "assertex" | "assertmsg" => Value::Undefined,
            "println" | "print" | "logstring" | "fprintln" | "prof_begin" | "prof_end" => Value::Undefined,
            "isint" => Value::Int(i32::from(matches!(args.first(), Some(Value::Int(_))))),
            "isfloat" => Value::Int(i32::from(matches!(args.first(), Some(Value::Float(_))))),
            "isvector" => Value::Int(i32::from(matches!(args.first(), Some(Value::Vector(_))))),
            "pointonsegmentnearesttopoint" => {
                let (a, b, p) = (v(args, 0)?, v(args, 1)?, v(args, 2)?);
                let ab = sub(b, a);
                let l2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
                let t = if l2 > 0.0 { ((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / l2 } else { 0.0 }.clamp(0.0, 1.0);
                Value::Vector([a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t])
            }
            _ => return Err(String::new()),
        })
    })())
    .filter(|r| !matches!(r, Err(e) if e.is_empty()))
}
