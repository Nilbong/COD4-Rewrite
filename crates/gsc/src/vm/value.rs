//! Script values. Arrays are values (copied on assignment, changed in place
//! through a variable or field); structs, entities and `level` are objects
//! shared by reference ([`ObjId`]).

use std::collections::HashMap;
use std::rc::Rc;

/// An object on the [`Heap`](super::Heap).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjId(pub u32);

/// A compiled function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Undefined,
    Int(i32),
    Float(f32),
    Str(Rc<str>),
    /// A localized string's key.
    IStr(Rc<str>),
    Vector([f32; 3]),
    Array(Rc<Array>),
    Object(ObjId),
    Func(FuncId),
    /// A builtin by name, taken as a function pointer (rare).
    Builtin(Rc<str>),
    /// `%name`, and the tree it's from.
    Anim(Rc<str>, Rc<str>),
    AnimTree(Rc<str>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Int(i32),
    Str(Rc<str>),
    Obj(ObjId),
}

/// An array: keys in the order added.
#[derive(Clone, Debug, Default)]
pub struct Array {
    entries: Vec<(Key, Value)>,
    index: HashMap<Key, usize>,
}

impl Array {
    pub fn get(&self, k: &Key) -> Option<&Value> {
        self.index.get(k).map(|&i| &self.entries[i].1)
    }

    pub fn set(&mut self, k: Key, v: Value) {
        if matches!(v, Value::Undefined) {
            self.remove(&k);
            return;
        }
        match self.index.get(&k) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                self.index.insert(k.clone(), self.entries.len());
                self.entries.push((k, v));
            }
        }
    }

    pub fn get_mut_or_insert(&mut self, k: Key) -> &mut Value {
        let i = match self.index.get(&k) {
            Some(&i) => i,
            None => {
                self.index.insert(k.clone(), self.entries.len());
                self.entries.push((k, Value::Undefined));
                self.entries.len() - 1
            }
        };
        &mut self.entries[i].1
    }

    pub fn remove(&mut self, k: &Key) {
        if let Some(i) = self.index.remove(k) {
            self.entries.remove(i);
            for (_, j) in self.index.iter_mut() {
                if *j > i {
                    *j -= 1;
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `getarraykeys`: newest first, as CoD4 returns them.
    pub fn keys(&self) -> impl Iterator<Item = &Key> {
        self.entries.iter().rev().map(|e| &e.0)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|e| &e.1)
    }

    pub fn push(&mut self, v: Value) {
        let n = self.len() as i32;
        self.set(Key::Int(n), v);
    }

    pub fn from_values(values: impl IntoIterator<Item = Value>) -> Array {
        let mut a = Array::default();
        for v in values {
            a.push(v);
        }
        a
    }
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }

    pub fn array(values: impl IntoIterator<Item = Value>) -> Value {
        Value::Array(Rc::new(Array::from_values(values)))
    }

    pub fn is_defined(&self) -> bool {
        !matches!(self, Value::Undefined)
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Undefined => false,
            Value::Int(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Str(s) | Value::IStr(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Int(v) => Some(*v as f32),
            Value::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Float(v) => Some(*v as i32),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::IStr(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_vec(&self) -> Option<[f32; 3]> {
        match self {
            Value::Vector(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<ObjId> {
        match self {
            Value::Object(o) => Some(*o),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Array> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn key(&self) -> Option<Key> {
        match self {
            Value::Int(v) => Some(Key::Int(*v)),
            Value::Float(v) => Some(Key::Int(*v as i32)),
            Value::Str(s) | Value::IStr(s) => Some(Key::Str(s.clone())),
            Value::Object(o) => Some(Key::Obj(*o)),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::IStr(_) => "localized string",
            Value::Vector(_) => "vector",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Func(_) | Value::Builtin(_) => "function",
            Value::Anim(..) => "animation",
            Value::AnimTree(_) => "animtree",
        }
    }

    /// As a string joins it (`"wave" + 2`).
    pub fn display(&self) -> String {
        match self {
            Value::Undefined => "undefined".into(),
            Value::Int(v) => v.to_string(),
            Value::Float(v) => {
                let s = format!("{v}");
                s
            }
            Value::Str(s) | Value::IStr(s) => s.to_string(),
            Value::Vector(v) => format!("({}, {}, {})", v[0], v[1], v[2]),
            Value::Anim(n, _) => n.to_string(),
            other => other.type_name().into(),
        }
    }
}

impl Key {
    pub fn value(&self) -> Value {
        match self {
            Key::Int(v) => Value::Int(*v),
            Key::Str(s) => Value::Str(s.clone()),
            Key::Obj(o) => Value::Object(*o),
        }
    }
}
