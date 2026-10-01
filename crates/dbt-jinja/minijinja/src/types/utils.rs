use crate::compiler::instructions::Instruction;
use crate::types::iterable::IterableType;
use crate::types::list::ListType;
use crate::types::struct_::StructType;
use crate::types::Type;
use crate::value::argtypes::KwargsValues;
use crate::value::{Value, ValueKind};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// Returns the type of a value
pub fn infer_type_from_const_value(val: &Value) -> Type {
    match val.kind() {
        ValueKind::Number => {
            if val.is_integer() {
                Type::Integer(val.as_i64())
            } else {
                Type::Float
            }
        }

        ValueKind::Bool => Type::Bool,
        ValueKind::String => Type::String(val.as_str().map(|s| s.to_string())),
        ValueKind::Bytes => Type::Bytes,
        ValueKind::Seq => {
            let element_type = {
                if let Ok(iter) = val.try_iter() {
                    iter.map(|v| infer_type_from_const_value(&v))
                        .next()
                        .unwrap_or(Type::Any { hard: false })
                } else {
                    Type::Any { hard: false }
                }
            };
            Type::List(ListType::new(element_type))
        }
        ValueKind::Map => {
            if val.downcast_object_ref::<KwargsValues>().is_some() {
                if let Ok(map_iter) = val.try_iter() {
                    let mut ty_map = BTreeMap::new();
                    for k in map_iter {
                        // For maps, the iterator yields keys; get the value for each key
                        if let Some(key_str) = k.as_str() {
                            if let Ok(v) = val.get_item(&k) {
                                let value_ty = infer_type_from_const_value(&v);
                                ty_map.insert(key_str.to_string(), Box::new(value_ty));
                            } else {
                                // fallback to generic Value if value can't be retrieved
                                return Type::Kwargs(BTreeMap::default());
                            }
                        } else {
                            // fallback to generic Value if keys are not strings
                            return Type::Kwargs(BTreeMap::default());
                        }
                    }
                    Type::Kwargs(ty_map)
                } else {
                    Type::Kwargs(BTreeMap::default())
                }
            } else if let Ok(map_iter) = val.try_iter() {
                let mut ty_map = BTreeMap::new();
                for k in map_iter {
                    // For maps, the iterator yields keys; get the value for each key
                    if let Some(key_str) = k.as_str() {
                        if let Ok(v) = val.get_item(&k) {
                            let value_ty = infer_type_from_const_value(&v);
                            ty_map.insert(key_str.to_string(), value_ty);
                        } else {
                            // fallback to generic Value if value can't be retrieved
                            return Type::Any { hard: false };
                        }
                    } else {
                        return Type::Any { hard: false };
                    }
                }
                Type::Struct(StructType::new(ty_map))
            } else {
                Type::Any { hard: false }
            }
        }
        ValueKind::Iterable => Type::Iterable(IterableType::new(Type::Any { hard: false })),
        ValueKind::Plain => Type::Plain,
        ValueKind::None => Type::None,
        ValueKind::Undefined => Type::Undefined,
        ValueKind::Invalid => Type::Invalid,
    }
}

/// Gets the name of the instructions
pub fn instr_name(instr: &Instruction) -> &'static str {
    match instr {
        Instruction::Add(_) => "+",
        Instruction::Sub(_) => "-",
        Instruction::Mul(_) => "*",
        Instruction::Div(_) => "/",
        Instruction::Eq(_) => "==",
        Instruction::Ne(_) => "!=",
        Instruction::Lt(_) => "<",
        Instruction::Lte(_) => "<=",
        Instruction::Gt(_) => ">",
        Instruction::Gte(_) => ">=",
        Instruction::Rem(_) => "%",
        Instruction::Pow(_) => "**",
        Instruction::StringConcat(_) => "+",
        Instruction::In(_) => "in",
        _ => "Other",
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
/// A location in a file.
pub struct CodeLocation {
    /// The line number.
    pub line: u32,
    /// The column number.
    pub col: u32,
    /// The file path.
    pub file: PathBuf,
}

impl CodeLocation {
    /// Create a new code location.
    pub fn new(line: u32, col: u32, file: PathBuf) -> Self {
        Self { line, col, file }
    }
}

impl fmt::Display for CodeLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 && self.col == 0 {
            write!(f, "{}", self.file.display())
        } else {
            write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
        }
    }
}
