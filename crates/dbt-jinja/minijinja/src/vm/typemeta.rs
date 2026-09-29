use dashmap::DashMap;

use crate::compiler::cfg::CFG;
use crate::compiler::codegen::{TypeConstraintOperation, Variable};
use crate::compiler::instructions::Instruction;
use crate::compiler::tokens::Span;
use crate::compiler::typecheck::FunctionRegistry;
use crate::constants::{
    DBT_AND_ADAPTERS_NAMESPACE, DIALECT, ROOT_PACKAGE_NAME, TARGET_PACKAGE_NAME,
};
use crate::dispatch_object::dbt_and_adapters_namespace_for;
use crate::types::function::{LambdaType, UserDefinedFunctionType};
use crate::types::list::ListType;
use crate::types::struct_::StructType;
use crate::types::tuple::TupleType;
use crate::types::utils::{infer_type_from_const_value, instr_name};
use crate::types::DynObject;
use crate::types::Type;
use crate::value::ValueMap;
use crate::vm::listeners::{DefaultTypecheckingEventListener, TypecheckingEventListener};
use crate::{ErrorKind, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::Hash;
use std::ops::RangeBounds;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use std::{fmt, vec};

/// Adapter methods whose `Parse`-mode implementation independently fabricates
/// relation/table/column/schema-shaped data (rather than returning a trivial
/// `bool`/`none` placeholder). Used both at runtime (`dbt-adapter`'s
/// `Adapter::call_method`, which owns the canonical copy of this exact list)
/// to taint the returned value, and statically here (`Instruction::CallMethod`
/// handling, via `on_introspective_call`) to flag which macros transitively
/// reach an introspective call, so a whole macro call can be treated as an
/// opaque taint boundary regardless of which internal branch a given render
/// actually takes. Keep in sync with `dbt_adapter::adapter::INTROSPECTIVE_METHODS`.
///
/// TODO: fs#12847 -- this list is matched by method name only, regardless of
/// receiver (the `adapter` global has no static type today, so there's
/// nothing to check the receiver against). Most of these names are
/// dbt/adapter-specific enough that a collision with an unrelated object's
/// method is very unlikely, but `execute` is generic enough to be a real, if
/// low-probability, false-positive risk. Revisit scoping this to the actual
/// `adapter` receiver.
pub const INTROSPECTIVE_METHOD_NAMES: &[&str] = &[
    "execute",
    "get_relation",
    "get_columns_in_relation",
    "get_missing_columns",
    "get_relations_by_pattern",
    "list_schemas",
    "get_column_schema_from_query",
    "get_columns_in_select_sql",
    "build_catalog_from_show_tables_and_svv_columns",
    "get_relation_config",
    "get_relations_without_caching",
    "list_relations_without_caching",
    "get_bq_table",
    "describe_relation",
    "get_partitions_metadata",
];

#[derive(Clone, Debug)]
struct TypeWithConstraint {
    inner: Type,
    constraint: BTreeMap<Part, TypeWithConstraint>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Part {
    String(String),
    Subscript(String),
}

impl std::fmt::Display for TypeWithConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl From<Type> for TypeWithConstraint {
    fn from(type_: Type) -> Self {
        TypeWithConstraint {
            inner: type_,
            constraint: BTreeMap::new(),
        }
    }
}

impl TypeWithConstraint {
    fn get_attribute(
        &self,
        name: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<TypeWithConstraint, crate::Error> {
        if let Some(constraint) = self.constraint.get(&Part::String(name.to_string())) {
            Ok(constraint.clone())
        } else {
            self.inner
                .get_attribute(name, listener)
                .map(TypeWithConstraint::from)
        }
    }

    fn subscript(
        &self,
        index: &TypeWithConstraint,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<TypeWithConstraint, crate::Error> {
        if let Type::String(Some(idx)) = &index.inner {
            if let Some(constraint) = self.constraint.get(&Part::Subscript(idx.clone())) {
                Ok(constraint.clone())
            } else {
                self.inner
                    .subscript(&index.inner, listener)
                    .map(TypeWithConstraint::from)
            }
        } else {
            self.inner
                .subscript(&index.inner, listener)
                .map(TypeWithConstraint::from)
        }
    }

    fn is_subtype_of(&self, other: &TypeWithConstraint) -> bool {
        // TODO: do we need to check the constraint?
        self.inner.is_subtype_of(&other.inner)
    }

    fn union(&self, other: &TypeWithConstraint) -> TypeWithConstraint {
        TypeWithConstraint {
            inner: self.inner.union(&other.inner),
            constraint: BTreeMap::new(),
        }
    }

    fn can_binary_op_with(
        &self,
        other: &TypeWithConstraint,
        op: &'static str,
        registry: Arc<DashMap<String, Type>>,
    ) -> Option<TypeWithConstraint> {
        self.inner
            .can_binary_op_with(&other.inner, op, registry)
            .map(TypeWithConstraint::from)
    }

    fn can_compare_with(&self, other: &TypeWithConstraint, op: &'static str) -> bool {
        self.inner.can_compare_with(&other.inner, op)
    }

    fn is_condition(&self) -> bool {
        self.inner.is_condition()
    }

    fn is_any(&self) -> bool {
        self.inner.is_any()
    }

    fn is_namespace(&self) -> bool {
        self.inner.is_namespace()
    }

    fn call(
        &self,
        positional_args: &[Type],
        kwargs: &BTreeMap<String, Type>,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        self.inner.call(positional_args, kwargs, listener)
    }

    fn is_optional(&self) -> bool {
        self.inner.is_optional()
    }

    fn is_none(&self) -> bool {
        self.inner.is_none()
    }

    fn get_non_optional_type(&self) -> Type {
        self.inner.get_non_optional_type()
    }

    fn exclude(&self, other: &Type) -> Type {
        self.inner.exclude(other)
    }

    // Add convenient method to extract inner Type
    fn into_inner(self) -> Type {
        self.inner
    }

    #[allow(unconditional_recursion)]
    fn insert(
        &mut self,
        path: &[Part],
        type_: Type,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<(), crate::Error> {
        if let Some((item, rest)) = path.split_first() {
            if let Some(attribute_type) = self.constraint.get_mut(item) {
                attribute_type.insert(rest, type_, listener)?;
            } else {
                let mut attribute_type = match item {
                    Part::String(s) => self.get_attribute(s, listener.clone())?,
                    Part::Subscript(s) => {
                        let idx_type = TypeWithConstraint::from(Type::String(Some(s.clone())));
                        self.subscript(&idx_type, listener.clone())?
                    }
                };
                attribute_type.insert(rest, type_, listener)?;
                self.constraint.insert(item.clone(), attribute_type);
            }
        } else {
            self.inner = type_;
        }
        Ok(())
    }

    fn get_simple_name(&self) -> String {
        match &self.inner {
            Type::String(_) => "String".to_string(),
            Type::Integer(_) => "Integer".to_string(),
            Type::Float => "Float".to_string(),
            Type::Bool => "Bool".to_string(),
            Type::Bytes => "Bytes".to_string(),
            Type::TimeStamp => "TimeStamp".to_string(),
            Type::Tuple(_) => "Tuple".to_string(),
            Type::List(_) => "List".to_string(),
            Type::Struct(_) => "Struct".to_string(),
            Type::Iterable(_) => "Iterable".to_string(),
            Type::Dict(_) => "Dict".to_string(),
            Type::Plain => "Plain".to_string(),
            Type::None => "None".to_string(),
            Type::Undefined => "Undefined".to_string(),
            Type::Invalid => "Invalid".to_string(),
            Type::Exception => "Exception".to_string(),
            Type::Union(_) => "Union".to_string(),
            Type::Any { .. } => "Any".to_string(),
            Type::Kwargs(_) => "Kwargs".to_string(),
            Type::Frame => "Frame".to_string(),
            Type::Object(arg0) => {
                if arg0.downcast_ref::<LambdaType>().is_some() {
                    "Lambda".to_string()
                } else {
                    format!("{arg0:?}")
                }
            }
            Type::Column => "Column".to_string(),
            Type::Namespace(_) => "Namespace".to_string(),
        }
    }
}

/// symbol table mapping local variable names to their types
#[derive(Clone, Debug, Default)]
struct SymbolTable {
    builtins: Arc<DashMap<String, Type>>,
    locals: BTreeMap<String, TypeWithConstraint>,
    locals_definitions_location: BTreeMap<String, Vec<Span>>,
}

impl SymbolTable {
    fn new(builtins: Arc<DashMap<String, Type>>) -> Self {
        Self {
            builtins,
            locals: BTreeMap::new(),
            locals_definitions_location: BTreeMap::new(),
        }
    }

    fn get(
        &self,
        variable: impl Into<Variable>,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<TypeWithConstraint, crate::Error> {
        let variable = variable.into();
        match variable {
            Variable::String(name) => self
                .locals
                .get(&name)
                .cloned()
                .or_else(|| {
                    self.builtins
                        .get(&name)
                        .map(|type_| type_.value().clone().into())
                })
                .ok_or_else(|| {
                    crate::Error::new(
                        ErrorKind::InvalidOperation,
                        format!("Variable not found: {name}"),
                    )
                }),
            Variable::GetAttr(path) => {
                // The first element must be Part::String
                let base_name = match &path[0] {
                    Part::String(s) => s,
                    _ => {
                        return Err(crate::Error::new(
                            ErrorKind::InvalidOperation,
                            format!("Base variable must be a string: {:?}", path[0]),
                        ));
                    }
                };
                let mut type_ = self
                    .locals
                    .get(base_name)
                    .cloned()
                    .or_else(|| {
                        self.builtins
                            .get(base_name)
                            .map(|type_| type_.value().clone().into())
                    })
                    .ok_or_else(|| {
                        crate::Error::new(
                            ErrorKind::InvalidOperation,
                            format!("Variable not found: {base_name}"),
                        )
                    })?;
                for part in path.iter().skip(1) {
                    match part {
                        Part::String(attr) => {
                            type_ = type_.get_attribute(attr, listener.clone())?;
                        }
                        Part::Subscript(idx) => {
                            let idx_type =
                                TypeWithConstraint::from(Type::String(Some(idx.clone())));
                            type_ = type_.subscript(&idx_type, listener.clone())?;
                        }
                    }
                }
                Ok(type_)
            }
        }
    }

    fn insert(
        &mut self,
        variable: impl Into<Variable>,
        value: Type,
        listener: Rc<dyn TypecheckingEventListener>,
        span_location: Option<Span>,
    ) -> Result<(), crate::Error> {
        let variable = variable.into();
        match variable {
            Variable::String(name) => {
                self.locals.insert(name.clone(), value.into());
                if let Some(span_location) = span_location {
                    self.locals_definitions_location
                        .insert(name, vec![span_location]);
                }
                Ok(())
            }
            Variable::GetAttr(path) => {
                let type_ = match self.locals.get_mut(match &path[0] {
                    Part::String(s) => s,
                    _ => unreachable!(),
                }) {
                    Some(type_) => type_,
                    None => if let Some(type_) = self.builtins.get(match &path[0] {
                        Part::String(s) => s,
                        _ => unreachable!(),
                    }) {
                        self.locals.insert(
                            match &path[0] {
                                Part::String(s) => s.clone(),
                                _ => unreachable!(),
                            },
                            type_.value().clone().into(),
                        );
                        if let Some(span_location) = span_location {
                            self.locals_definitions_location.insert(
                                match &path[0] {
                                    Part::String(s) => s.clone(),
                                    _ => unreachable!(),
                                },
                                vec![span_location],
                            );
                        }
                        self.locals.get_mut(&match &path[0] {
                            Part::String(s) => s.clone(),
                            _ => unreachable!(),
                        })
                    } else {
                        None
                    }
                    .ok_or_else(|| {
                        crate::Error::new(
                            ErrorKind::InvalidOperation,
                            format!("Variable not found: {:?}", path[0]),
                        )
                    })?,
                };
                type_.insert(&path[1..], value, listener)?;
                Ok(())
            }
        }
    }

    fn keys(&self) -> impl Iterator<Item = &String> {
        self.locals.keys()
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut TypeWithConstraint> {
        self.locals.get_mut(name)
    }

    fn get_ref(&self, name: &str) -> Option<&TypeWithConstraint> {
        self.locals.get(name)
    }
}

#[derive(Clone, Debug, Default)]
struct TypecheckStack(Vec<TypeWithConstraint>);

impl TypecheckStack {
    fn push(&mut self, type_: impl Into<TypeWithConstraint>) {
        self.0.push(type_.into());
    }

    fn truncate(&mut self, n: usize) {
        self.0.truncate(n);
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn last(&self) -> Option<&TypeWithConstraint> {
        self.0.last()
    }

    fn drain<R>(&mut self, range: R) -> std::vec::Drain<'_, TypeWithConstraint>
    where
        R: RangeBounds<usize>,
    {
        self.0.drain(range)
    }

    fn pop(&mut self) -> Option<TypeWithConstraint> {
        self.0.pop()
    }

    // Add convenient method to pop inner Type directly
    fn pop_inner(&mut self) -> Option<Type> {
        self.0.pop().map(|t| t.inner)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn get(&self, index: usize) -> Option<&TypeWithConstraint> {
        self.0.get(index)
    }
}

/// The states of the type checker
#[derive(Clone, Debug)]
struct TypecheckState {
    stack: TypecheckStack,
    locals: SymbolTable,
    frame_base: usize,
    cur_loop_obj_type: Option<Type>,
    single_branch_definition_vars: BTreeSet<String>,
    rv_type: Type,
    return_span: Span,
}

impl TypecheckState {
    fn new(builtins: Arc<DashMap<String, Type>>) -> Self {
        TypecheckState {
            stack: TypecheckStack::default(),
            locals: SymbolTable::new(builtins),
            frame_base: 0,
            cur_loop_obj_type: None,
            single_branch_definition_vars: BTreeSet::new(),
            rv_type: Type::None,
            return_span: Span::default(),
        }
    }

    fn drop_top(&mut self, n: usize) {
        self.stack.truncate(self.stack.len().saturating_sub(n));
    }

    #[track_caller]
    fn peek(&self) -> &TypeWithConstraint {
        self.stack.last().unwrap()
    }

    fn push_frame(&mut self) {
        self.frame_base = self.stack.len();
    }

    fn get_call_args(&mut self, n: u16) -> (Vec<Type>, BTreeMap<String, Type>) {
        // get n items from the stack
        let all_args = self
            .stack
            .drain(self.stack.len().saturating_sub(n as usize)..)
            .collect::<Vec<_>>();
        if let Some(Type::Kwargs(kwargs)) = all_args.last().cloned().map(|t| t.inner) {
            let len = all_args.len();
            (
                all_args
                    .into_iter()
                    .take(len - 1)
                    .map(|t| t.inner)
                    .collect::<Vec<_>>(),
                kwargs
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_ref().clone()))
                    .collect(),
            )
        } else {
            (
                all_args.into_iter().map(|t| t.inner).collect::<Vec<_>>(),
                BTreeMap::new(),
            )
        }
    }
}

/// CFG-based type checker
pub struct TypeChecker<'src> {
    instr: &'src [Instruction<'src>], // TODO: put instr and &function_registry into in_states
    cfg: CFG,
    in_states: Vec<TypecheckState>,
    function_registry: Arc<FunctionRegistry>,
    builtins: Arc<DashMap<String, Type>>,
}

/// Typecheck logic implementation
impl<'src> TypeChecker<'src> {
    pub fn new(
        instr: &'src [Instruction<'src>],
        cfg: CFG,
        funcsigns: Arc<FunctionRegistry>,
        builtins: Arc<DashMap<String, Type>>,
    ) -> Self {
        let in_states = vec![TypecheckState::new(builtins.clone()); cfg.blocks.len()];
        Self {
            instr,
            cfg,
            in_states,
            function_registry: funcsigns,
            builtins,
        }
    }

    pub fn check(
        &mut self,
        listener: Rc<dyn TypecheckingEventListener>,
        typecheck_resolved_context: BTreeMap<String, Value>,
    ) -> Result<(), crate::Error> {
        let mut worklist = VecDeque::new();
        let mut visited = vec![false; self.cfg.blocks.len()];
        let mut first_merge = vec![true; self.cfg.blocks.len()];

        // Find all roots (blocks with no predecessors)
        for (i, block) in self.cfg.blocks.iter().enumerate() {
            if block.predecessor.is_empty() {
                self.in_states[i] = TypecheckState::new(self.builtins.clone());
                worklist.push_back(i);
                visited[i] = true;
                first_merge[i] = false;
            }
        }

        while let Some(bb_id) = worklist.pop_front() {
            listener.clone().new_block(bb_id);
            let out_state =
                self.transfer_block(bb_id, listener.clone(), typecheck_resolved_context.clone())?;

            let rv_type = out_state.rv_type.clone();
            if let Some(macro_block) = self.cfg.get_block(bb_id) {
                if let Some(macro_name) = macro_block.current_macro.as_ref() {
                    if let Some(funcsign) = self.function_registry.get(macro_name) {
                        if let Some(user_defined_func) =
                            funcsign.downcast_ref::<UserDefinedFunctionType>()
                        {
                            let expected_ret_type = user_defined_func.ret_type.clone();
                            // try match rv with registry_ret_type
                            let span = out_state.return_span;
                            if !rv_type.is_compatible_with(&expected_ret_type) {
                                listener.set_span(&span);
                                listener.warn(
                                    &format!(
                                        "Type mismatch: expected return type {expected_ret_type}, got {rv_type}"
                                    ),
                                );
                            }
                        }
                    }
                }
            }

            for (succ, _) in self.cfg.successor(bb_id) {
                let changed = if first_merge[*succ] {
                    self.in_states[*succ] = out_state.clone();
                    first_merge[*succ] = false;
                    true
                } else {
                    Self::merge_into(
                        &mut self.in_states[*succ],
                        &out_state,
                        visited[*succ],
                        listener.clone(),
                    )
                };
                if !visited[*succ] || changed {
                    worklist.push_back(*succ);
                    visited[*succ] = true;
                }
            }
            if let Some(macro_block) = self.cfg.get_block(bb_id) {
                // find the last block in a macro
                if macro_block.successor.is_empty() {
                    if let Some(macro_name) = macro_block.current_macro.as_ref() {
                        if let Some(funcsign) = self.function_registry.get(macro_name) {
                            if let Some(user_defined_func) =
                                funcsign.downcast_ref::<UserDefinedFunctionType>()
                            {
                                let expected_ret_type = user_defined_func.ret_type.clone();
                                if !expected_ret_type.is_compatible_with(&Type::String(None)) {
                                    listener.set_span(&macro_block.span.unwrap_or_default());
                                    listener.warn(
                                        &format!("Type mismatch: expected return type {expected_ret_type}, got String"),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The internal function typechecking a single block.
    #[allow(clippy::too_many_arguments)]
    fn transfer_block(
        &mut self,
        bb_id: usize,
        listener: Rc<dyn TypecheckingEventListener>,
        typecheck_resolved_context: BTreeMap<String, Value>,
    ) -> Result<TypecheckState, crate::Error> {
        let suppressed_listener = Rc::new(DefaultTypecheckingEventListener::default());
        let mut typestate = self.in_states[bb_id].clone();
        let slice = self.cfg.instructions(bb_id, self.instr);
        let attempts: &mut Vec<std::string::String> = &mut Vec::new();

        for (offset, inst) in slice.iter().enumerate() {
            let global_idx = self.cfg.blocks[bb_id].start + offset;

            match inst {
                Instruction::Swap => {
                    // TYPECHECK: NO
                    let a = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on swap",
                            ))
                        }
                    };
                    let b = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on swap",
                            ))
                        }
                    };
                    typestate.stack.push(b);
                    typestate.stack.push(a);
                }
                Instruction::EmitRaw(_, _) => {
                    // TYPECHECK: NO
                    // no need to update the type stack
                }
                Instruction::Emit(_) => {
                    // TYPECHECK: NO
                    let _item1 = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on emit",
                            ));
                        }
                    };
                }
                Instruction::StoreLocal(name, span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                    let value_type = match typestate.stack.pop() {
                        Some(val) => {
                            if *name != "_internal_tmp"
                                && macro_namespace_template_resolver(
                                    &typecheck_resolved_context,
                                    self.function_registry.clone(),
                                    name,
                                    attempts,
                                )
                                .is_none()
                            {
                                listener.on_lookup(
                                    span,
                                    &val.get_simple_name(),
                                    &format!("{val}"),
                                    vec![*span],
                                );
                            }
                            val.inner
                        }
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                format!("Stack underflow on store local {name} {span:?}"),
                            ));
                        }
                    };
                    let block = self.cfg.get_block(bb_id).unwrap();
                    let span_location = if let Some(macro_name) = &block.current_macro {
                        if *name == macro_name {
                            None
                        } else {
                            Some(*span)
                        }
                    } else {
                        None
                    };
                    typestate.locals.insert(
                        (*name).to_string(),
                        value_type.clone(),
                        listener.clone(),
                        span_location,
                    )?;
                }
                Instruction::Lookup(name, span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                    let name_str: &str = name;
                    // first try to search in self.cfg.get_block(bb_id).type_narrow
                    if let Ok(ty) = typestate.locals.get(name_str, listener.clone()) {
                        if typestate.single_branch_definition_vars.contains(name_str) {
                            listener.warn(
                                &format!("Variable '{name_str}' is not defined in one of its predecessor blocks."),
                            );
                            typestate.stack.push(Type::Any { hard: false });
                            if name_str != "_internal_tmp" {
                                // get the spans from locals_definitions_location
                                if let Some(spans) =
                                    typestate.locals.locals_definitions_location.get(name_str)
                                {
                                    listener.on_lookup(span, "any", "any", spans.clone());
                                }
                            }
                        } else {
                            typestate.stack.push(ty.clone());
                            if name_str != "_internal_tmp" {
                                if let Some(spans) =
                                    typestate.locals.locals_definitions_location.get(name_str)
                                {
                                    listener.on_lookup(
                                        span,
                                        &ty.get_simple_name(),
                                        &format!("{ty}"),
                                        spans.clone(),
                                    );
                                }
                            }
                        }
                    } else if let Some(function) = self.function_registry.get(name_str) {
                        typestate.stack.push(Type::Object(function.clone()));
                    } else {
                        listener.warn(&format!(
                            "Potential TypeError: Unknown local variable '{name_str}'"
                        ));
                        typestate.stack.push(Type::Any { hard: false });
                    }
                }
                Instruction::GetAttr(name, span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // pop a type from the stack
                    let value_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on get attr",
                            ))
                        }
                    };
                    typestate
                        .stack
                        .push(value_type.get_attribute(name, listener.clone())?);
                }

                Instruction::SetAttr(_name, _span) => {
                    // TYPECHECK: NO
                    let _item1 = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on set attr",
                            ))
                        }
                    };
                    let _item2 = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on set attr",
                            ))
                        }
                    };
                }
                Instruction::GetItem(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    let index = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on get item",
                            ))
                        }
                    };
                    let base = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on get item",
                            ))
                        }
                    };
                    typestate
                        .stack
                        .push(base.subscript(&index, listener.clone())?);
                }
                Instruction::Slice(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // b, step, stop must be Integer, None, or Value (or a union containing any of these)
                    let step = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on slice",
                            ))
                        }
                    };
                    let stop = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on slice",
                            ))
                        }
                    };
                    let _ = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on slice",
                            ))
                        }
                    };
                    let b = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on slice",
                            ))
                        }
                    };

                    for (name, slice_type) in [("b", &b), ("stop", &stop), ("step", &step)] {
                        if !slice_type.is_subtype_of(&Type::Integer(None).into()) {
                            listener.warn(&format!(
                                "Type mismatch for slice {name}: type = {slice_type}"
                            ));
                        }
                    }

                    typestate.stack.push(Type::Any { hard: false });
                }
                Instruction::LoadConst(val) => {
                    // TYPECHECK: NO
                    typestate.stack.push(infer_type_from_const_value(val));
                }
                Instruction::BuildMap(pair_count, span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                    let mut args_map_types = vec![];
                    for _ in 0..*pair_count {
                        let v = match typestate.stack.pop_inner() {
                            Some(val) => val,
                            None => {
                                return Err(crate::Error::new(
                                    crate::error::ErrorKind::InvalidOperation,
                                    "Stack underflow on build map value",
                                ))
                            }
                        };
                        let k = match typestate.stack.pop() {
                            Some(val) => val,
                            None => {
                                return Err(crate::Error::new(
                                    crate::error::ErrorKind::InvalidOperation,
                                    "Stack underflow on build map key",
                                ))
                            }
                        };
                        args_map_types.push((k, v));
                    }
                    let mut args_map = BTreeMap::new();
                    let mut success = true;
                    for (k, v) in args_map_types {
                        if let TypeWithConstraint {
                            inner: Type::String(Some(k)),
                            ..
                        } = k
                        {
                            args_map.insert(k, v);
                        } else {
                            success = false;
                            break;
                        }
                    }
                    if success {
                        typestate
                            .stack
                            .push(Type::Struct(StructType::new(args_map)));
                    } else {
                        typestate.stack.push(Type::Any { hard: true });
                    }
                }
                Instruction::BuildKwargs(pair_count) => {
                    // TYPECHECK: NO
                    let mut args_map = BTreeMap::new();
                    for _ in 0..*pair_count {
                        let value = match typestate.stack.pop_inner() {
                            Some(val) => val,
                            None => Type::Any { hard: false },
                        };
                        let key = match typestate.stack.pop_inner() {
                            Some(val) => val,
                            None => Type::Any { hard: false },
                        };
                        if let Type::String(Some(key)) = key {
                            args_map.insert(key.to_string(), Box::new(value));
                        }
                    }
                    typestate.stack.push(Type::Kwargs(args_map));
                }
                Instruction::MergeKwargs(count) => {
                    // TYPECHECK: NO
                    let mut args_map = BTreeMap::new();
                    for _ in 0..*count {
                        let kwargs = match typestate.stack.pop_inner() {
                            Some(val) => val,
                            None => {
                                return Err(crate::Error::new(
                                    crate::error::ErrorKind::InvalidOperation,
                                    "Stack underflow on merge kwargs",
                                ))
                            }
                        };
                        // get the map from the kwargs type
                        if let Type::Kwargs(kwargs_map) = kwargs {
                            for (k, v) in kwargs_map {
                                args_map.insert(k, v);
                            }
                        }
                    }
                    typestate.stack.push(Type::Kwargs(args_map));
                }
                Instruction::BuildList(n, span) => {
                    listener.set_span(span);

                    let count = n.unwrap_or(0);
                    if count == 0 {
                        typestate
                            .stack
                            .push(Type::List(ListType::new(Type::Any { hard: true })));
                    } else {
                        // Collect the types of the items to be popped
                        if let Some(mut item_type) = typestate.stack.pop() {
                            for _ in 1..count {
                                let other = match typestate.stack.pop() {
                                    Some(val) => val,
                                    None => {
                                        return Err(crate::Error::new(
                                            crate::error::ErrorKind::InvalidOperation,
                                            "Stack underflow on build list",
                                        ))
                                    }
                                };
                                item_type = item_type.union(&other);
                            }
                            typestate
                                .stack
                                .push(Type::List(ListType::new(item_type.into_inner())));
                        } else {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on build list",
                            ));
                        }
                    }
                }
                Instruction::BuildTuple(n, span) => {
                    listener.set_span(span);
                    if let Some(n) = n {
                        let mut item_types = Vec::new();
                        for _ in 0..*n {
                            let item_type = match typestate.stack.pop_inner() {
                                Some(val) => val,
                                None => {
                                    return Err(crate::Error::new(
                                        crate::error::ErrorKind::InvalidOperation,
                                        "Stack underflow on build tuple",
                                    ));
                                }
                            };
                            item_types.push(item_type);
                        }
                        item_types.reverse();
                        typestate
                            .stack
                            .push(Type::Tuple(TupleType::new(item_types)));
                    } else {
                        listener.warn(
                            "Type mismatch for build tuple: expected tuple with a fixed number of elements, got None",
                        );
                        typestate.stack.push(Type::Any { hard: false });
                    }
                }
                Instruction::UnpackList(count, span) => {
                    listener.set_span(span);
                    let tuple_type = match typestate.stack.pop_inner() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on unpack list",
                            ))
                        }
                    };
                    // if tuple_type is not a Tuple, we have a type error
                    match &tuple_type {
                        Type::Tuple(tuple) if tuple.fields.len() == *count => {
                            for field_type in tuple.fields.iter().rev() {
                                typestate.stack.push(field_type.clone());
                            }
                        }
                        Type::List(list_type) => {
                            // get list_type.element
                            let element_type = list_type.element.clone();
                            for _ in 0..*count {
                                typestate.stack.push(*element_type.clone());
                            }
                        }
                        _ => {
                            for _ in 0..*count {
                                typestate.stack.push(Type::Any { hard: false });
                            }
                            listener.warn(&format!(
                                "Type mismatch for unpack list: expected Tuple with {count} elements, got {tuple_type}"
                            ));
                        }
                    };
                }
                Instruction::UnpackLists(_count, _span) => {
                    // TODO
                    // We need to modify the structure of the UnpackLists instruction, adding an expected total items count
                }
                Instruction::Add(span)
                | Instruction::Sub(span)
                | Instruction::Mul(span)
                | Instruction::Div(span)
                | Instruction::IntDiv(span)
                | Instruction::Pow(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // lhs and rhs must have the same type
                    let op = instr_name(&self.instr[global_idx]);
                    let rhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on binary operation",
                            ))
                        }
                    };
                    let lhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on binary operation",
                            ))
                        }
                    };

                    let result_type =
                        lhs_type.can_binary_op_with(&rhs_type, op, self.builtins.clone());
                    if let Some(result_type) = result_type {
                        typestate.stack.push(result_type);
                    } else {
                        listener.warn(&format!(
                            "Type mismatch for {op}: lhs = {lhs_type}, rhs = {rhs_type}"
                        ));
                        typestate.stack.push(Type::Any { hard: false });
                    }
                }

                Instruction::Rem(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // lhs and rhs must have the same type
                    // or, according to the runtime logic, Rem can be used with lhs = String, rhs = Seq
                    let op = instr_name(&self.instr[global_idx]);

                    let rhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on rem operation",
                            ))
                        }
                    };
                    let lhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on rem operation",
                            ))
                        }
                    };

                    // Check for string formatting case: lhs = String, rhs = Seq
                    if !lhs_type.is_subtype_of(&Type::String(None).into())
                        && rhs_type.is_subtype_of(
                            &Type::List(ListType {
                                element: Box::new(Type::None),
                            })
                            .into(),
                        )
                    {
                        typestate.stack.push(Type::String(None));
                        continue;
                    }

                    let result_type =
                        lhs_type.can_binary_op_with(&rhs_type, op, self.builtins.clone());
                    if let Some(result_type) = result_type {
                        typestate.stack.push(result_type);
                    } else {
                        listener.warn(&format!(
                            "Type mismatch for {op}: lhs = {lhs_type}, rhs = {rhs_type}"
                        ));
                        typestate.stack.push(Type::Any { hard: false });
                    }
                }

                Instruction::Eq(span)
                | Instruction::Ne(span)
                | Instruction::Lt(span)
                | Instruction::Lte(span)
                | Instruction::Gt(span)
                | Instruction::Gte(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // lhs and rhs must have the same type
                    let op = instr_name(&self.instr[global_idx]);
                    let rhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on binary operation",
                            ))
                        }
                    };
                    let lhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on binary operation",
                            ))
                        }
                    };

                    let result_type = lhs_type.can_compare_with(&rhs_type, op);
                    if !result_type {
                        listener.warn(&format!(
                            "Type mismatch for {op}: lhs = {lhs_type}, rhs = {rhs_type}"
                        ));
                    }
                    typestate.stack.push(Type::Bool);
                }

                Instruction::Not(span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                    let item_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on not operation",
                            ))
                        }
                    };
                    if item_type.is_optional() {
                        typestate.stack.push(item_type.exclude(&Type::None));
                    } else {
                        typestate.stack.push(Type::Bool);
                    }
                }
                Instruction::StringConcat(span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                    // Stringconcat can actually concat any two types
                    let rhs_type = match typestate.stack.pop_inner() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on string concat operation",
                            ))
                        }
                    };
                    let lhs_type = match typestate.stack.pop_inner() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on string concat operation",
                            ))
                        }
                    };

                    typestate.stack.push(Type::String(
                        if let (Type::String(Some(lhs_value)), Type::String(Some(rhs_value))) =
                            (lhs_type, rhs_type)
                        {
                            Some(format!("{lhs_value}{rhs_value}"))
                        } else {
                            None
                        },
                    ));
                }
                Instruction::In(span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                    let _rhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on in operation",
                            ))
                        }
                    };
                    let _lhs_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on in operation",
                            ))
                        }
                    };

                    typestate.stack.push(Type::Bool);
                }
                Instruction::Neg(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // The operand must be a number
                    let a = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on negation",
                            ))
                        }
                    };

                    // TODO impl a.neg()
                    typestate.stack.push(a);
                }
                Instruction::Pos(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // The operand must be a number
                    let a = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on unary plus",
                            ))
                        }
                    };

                    typestate.stack.push(a);
                }
                Instruction::PushWith(_) => {
                    // TYPECHECK: NO
                    typestate.push_frame();
                }
                Instruction::PopFrame => {
                    // TYPECHECK: NO
                    typestate.stack.truncate(typestate.frame_base);

                    typestate.cur_loop_obj_type = None;

                    let maybe_capture = false;
                    if maybe_capture {
                        typestate.stack.push(Type::Any { hard: false });
                    }

                    typestate.frame_base = typestate.stack.len();
                }
                #[cfg(feature = "macros")]
                Instruction::IsUndefined => {
                    // TYPECHECK: NO
                    typestate.stack.pop();

                    typestate.stack.push(Type::Bool);
                }
                Instruction::PushLoop(_flags, span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                    if let Some(iterable) = typestate.stack.pop_inner() {
                        let element_type = match iterable {
                            Type::List(list) => *list.element.clone(),
                            Type::Iterable(iterable) => *iterable.element.clone(),
                            Type::Dict(dict) => *dict.key.clone(),
                            Type::Any { hard: true } => Type::Any { hard: true },

                            _ => {
                                let func = iterable.get_attribute("__iter__", listener.clone())?;
                                func.call(&[], &BTreeMap::new(), listener.clone())?
                            }
                        };
                        typestate.cur_loop_obj_type = Some(element_type);
                    } else {
                        return Err(crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            "Stack underflow on push loop",
                        ));
                    }
                    typestate.push_frame();
                }
                Instruction::Iterate(_jump_target, _span) => {
                    // TYPECHECK: NO
                    if let Some(element_type) = typestate.cur_loop_obj_type.clone() {
                        typestate.stack.push(element_type);
                    } else {
                        return Err(crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            "current loop object type is not set",
                        ));
                    }
                }
                Instruction::PushDidNotIterate => {
                    // TYPECHECK: NO
                    typestate.stack.push(Type::Any { hard: false });
                }
                Instruction::Jump(_jump_target, _) => {
                    // TYPECHECK: NO
                    // have nothing to do with the stack
                }
                Instruction::JumpIfFalse(_else_label, _span) => {
                    let _item_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on jump if false",
                            ))
                        }
                    };
                }
                Instruction::JumpIfTrue(_else_label, _) => {
                    let _item_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on jump if false",
                            ))
                        }
                    };
                }
                Instruction::JumpIfFalseOrPop(_jump_target, span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // the operand must be a boolean
                    let a = typestate.peek().clone();

                    if !a.is_condition() {
                        listener.warn(&format!("Type mismatch for jump condition: type = {a}"));
                    }
                }
                Instruction::JumpIfTrueOrPop(_jump_target, span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // the operand must be a boolean
                    let a = typestate.peek().clone();

                    if !a.is_condition() {
                        listener.warn(&format!("Type mismatch for jump condition: type = {a}"));
                    }
                }
                #[cfg(feature = "multi_template")]
                Instruction::CallBlock(_name) => {
                    // TYPECHECK: NO
                    let saved_base = typestate.stack.len();
                    // truncate
                    typestate.stack.truncate(saved_base);
                }
                Instruction::PushAutoEscape(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // the operand must be a string
                    let a = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on push auto escape",
                            ))
                        }
                    };

                    if !a.is_subtype_of(&Type::String(None).into()) {
                        listener.warn(&format!("Type mismatch for auto escape: type = {a}"));
                    }
                }
                Instruction::PopAutoEscape => {
                    // TYPECHECK: NO
                    // nothing to do with the stack
                }
                Instruction::BeginCapture(_mode) => {
                    // TYPECHECK: NO
                    // nothing to do with the stack
                }
                Instruction::EndCapture => {
                    // TYPECHECK: NO
                    typestate.stack.push(Type::String(None));
                }
                Instruction::ApplyFilter(name, arg_count, _local_id, span) => {
                    // TYPECHECK: YES - filter types are registered in function_registry
                    listener.set_span(span);

                    if let Some(arg_cnt) = arg_count {
                        let (args, kwargs) = typestate.get_call_args(*arg_cnt);

                        // Try to call the filter if it's registered in locals
                        if let Ok(Type::Object(funcsign)) = typestate
                            .locals
                            .get(name, listener.clone())
                            .map(|t| t.inner)
                        {
                            let funcsign = funcsign.clone();
                            typestate.stack.push(funcsign.call(
                                &args,
                                &kwargs,
                                listener.clone(),
                            )?);
                        } else if let Some(funcsign) = self.function_registry.get(name.to_owned()) {
                            // Try function_registry as fallback (for built-in filters like as_bool, as_number, etc.)
                            typestate.stack.push(funcsign.call(
                                &args,
                                &kwargs,
                                listener.clone(),
                            )?);
                        } else {
                            // Filter not registered - warn and push Any type
                            listener.warn(&format!(
                                "Potential TypeError: Filter '{name}' is not defined."
                            ));
                            typestate.stack.push(Type::Any { hard: false });
                        }
                    } else {
                        // TODO: handle the case when arg_count is None
                        listener.warn(&format!(
                            "Potential TypeError: Filter '{name}' requires arguments."
                        ));
                        typestate.stack.push(Type::Any { hard: false });
                    }
                }
                Instruction::PerformTest(_name, arg_count, _local_id, _span) => {
                    // TYPECHECK: NO
                    typestate.drop_top(arg_count.unwrap_or(0) as usize);
                    typestate.stack.push(Type::Bool);
                }
                Instruction::CallFunction(
                    name,
                    arg_count,
                    identifier_span,
                    span,
                    ref_or_source_span,
                ) => {
                    // TYPECHECK: YES
                    listener.set_span(span);

                    if *name == "caller" {
                        // judge whether current block is a macro
                        if let Some(block) = self.cfg.get_block(bb_id) {
                            if let Some(_macro_name) = &block.current_macro {
                                if let Some(arg_cnt) = arg_count {
                                    let (args, kwargs) = typestate.get_call_args(*arg_cnt);

                                    typestate.stack.push(
                                        self.builtins.get("caller").unwrap().call(
                                            &args,
                                            &kwargs,
                                            listener.clone(),
                                        )?,
                                    );
                                } else {
                                    return Err(crate::Error::new(
                                        crate::error::ErrorKind::InvalidOperation,
                                        "Function 'caller' requires an argument count",
                                    ));
                                }
                            } else {
                                return Err(crate::Error::new(
                                    crate::error::ErrorKind::InvalidOperation,
                                    "Function 'caller' requires a macro block",
                                ));
                            }
                        }
                    } else if *name == "source" || *name == "ref" {
                        if let Some(arg_cnt) = arg_count {
                            let (args, kwargs) = typestate.get_call_args(*arg_cnt);
                            let function_type = match *name {
                                "source" => self.builtins.get("source").unwrap(),
                                "ref" => self.builtins.get("ref").unwrap(),
                                _ => unreachable!(),
                            };
                            typestate.stack.push(function_type.call(
                                &args,
                                &kwargs,
                                listener.clone(),
                            )?);

                            if *name == "ref" {
                                if let Some(ref_or_source_span) = ref_or_source_span {
                                    if let Type::String(Some(name)) = &args[0] {
                                        listener.on_model_reference(
                                            name,
                                            identifier_span,
                                            &ref_or_source_span.start_line,
                                            &ref_or_source_span.start_col,
                                            &ref_or_source_span.start_offset,
                                            &ref_or_source_span.end_line,
                                            &ref_or_source_span.end_col,
                                            &ref_or_source_span.end_offset,
                                        );
                                    }
                                }
                            } else if *name == "source" && (args.len() == 2) {
                                if let Some(ref_or_source_span) = ref_or_source_span {
                                    if let (
                                        Type::String(Some(source_name)),
                                        Type::String(Some(model_name)),
                                    ) = (args.first().unwrap(), args.last().unwrap())
                                    {
                                        listener.on_model_source_reference(
                                            source_name,
                                            model_name,
                                            identifier_span,
                                            &ref_or_source_span.start_line,
                                            &ref_or_source_span.start_col,
                                            &ref_or_source_span.start_offset,
                                            &ref_or_source_span.end_line,
                                            &ref_or_source_span.end_col,
                                            &ref_or_source_span.end_offset,
                                        );
                                    }
                                }
                            }
                        }
                    } else if let Ok(Type::Object(funcsign)) = typestate
                        .locals
                        .get(name, listener.clone())
                        .map(|t| t.inner)
                    {
                        if let Some(arg_cnt) = arg_count {
                            let funcsign = funcsign.clone();
                            let (args, kwargs) = typestate.get_call_args(*arg_cnt);

                            typestate.stack.push(funcsign.call(
                                &args,
                                &kwargs,
                                listener.clone(),
                            )?);
                            if let (Some(def_span), Some(def_path), Some(def_unique_id)) = (
                                funcsign.get_span(),
                                funcsign.get_path(),
                                funcsign.get_unique_id(),
                            ) {
                                listener.on_function_call(
                                    identifier_span,
                                    &def_span,
                                    &def_path,
                                    &def_unique_id,
                                );
                            }
                        }
                    } else if let Ok(Type::Any { hard: true }) = typestate
                        .locals
                        .get(name, listener.clone())
                        .map(|t| t.inner)
                    {
                        typestate.stack.push(Type::Any { hard: true });
                    } else if let Some(funcsign) = self.function_registry.get(name.to_owned()) {
                        if let Some(arg_cnt) = arg_count {
                            let (args, kwargs) = typestate.get_call_args(*arg_cnt);

                            typestate.stack.push(funcsign.call(
                                &args,
                                &kwargs,
                                listener.clone(),
                            )?);
                            if let (Some(def_span), Some(def_path), Some(def_unique_id)) = (
                                funcsign.get_span(),
                                funcsign.get_path(),
                                funcsign.get_unique_id(),
                            ) {
                                listener.on_function_call(
                                    identifier_span,
                                    &def_span,
                                    &def_path,
                                    &def_unique_id,
                                );
                            }
                        }
                    } else if let Some(template_name) = macro_namespace_template_resolver(
                        &typecheck_resolved_context,
                        self.function_registry.clone(),
                        name,
                        attempts,
                    ) {
                        if let Some(funcsign) = self.function_registry.get(&template_name) {
                            if let Some(arg_cnt) = arg_count {
                                let (args, kwargs) = typestate.get_call_args(*arg_cnt);

                                typestate.stack.push(funcsign.call(
                                    &args,
                                    &kwargs,
                                    listener.clone(),
                                )?);
                                if let (Some(def_span), Some(def_path), Some(def_unique_id)) = (
                                    funcsign.get_span(),
                                    funcsign.get_path(),
                                    funcsign.get_unique_id(),
                                ) {
                                    listener.on_function_call(
                                        identifier_span,
                                        &def_span,
                                        &def_path,
                                        &def_unique_id,
                                    );
                                }
                            }
                        }
                    } else if let Some(arg_cnt) = arg_count {
                        let _ = typestate.get_call_args(*arg_cnt);
                        // Don't warn for known built-in functions
                        let known_functions = ["doc", "var"];
                        if !known_functions.contains(name) {
                            listener.warn(&format!(
                                "Potential TypeError: Function '{name}' is not defined."
                            ));
                        }
                        typestate.stack.push(Type::Any { hard: false });
                    } else {
                        // TODO: handle the case when arg_count is None
                        // Don't warn for known built-in functions
                        let known_functions = ["doc", "var"];
                        if !known_functions.contains(name) {
                            listener.warn(&format!(
                                "Potential TypeError: Function '{name}' is not defined."
                            ));
                        }
                        typestate.stack.push(Type::Any { hard: false });
                    }
                }
                Instruction::CallMethod(name, arg_count, identifier_span, span, _) => {
                    // TYPECHECK: NO? (Maybe add method check later)
                    listener.set_span(span);

                    // See `on_introspective_call`'s doc comment: this is a
                    // name-only, receiver-type-independent check (the
                    // `adapter` global has no static type today), which is
                    // intentionally conservative.
                    if INTROSPECTIVE_METHOD_NAMES.contains(name) {
                        listener.on_introspective_call(name);
                    }

                    let count = arg_count.unwrap_or(0);
                    if count > 0 {
                        // Pop (arg_count - 1) arguments
                        let (method_args, kwargs) = typestate.get_call_args(count - 1);
                        // Pop the last one as 'self'
                        let self_type = match typestate.stack.pop() {
                            Some(val) => val,
                            None => {
                                return Err(crate::Error::new(
                                    crate::error::ErrorKind::InvalidOperation,
                                    "Stack underflow on call method",
                                ))
                            }
                        };

                        if self_type.is_namespace() {
                            let namespace_name = match self_type.inner.clone() {
                                Type::Namespace(name) => name,
                                _ => unreachable!(),
                            };
                            let qualified_name = format!("{namespace_name}.{name}");
                            if let Some(funcsign) = self.function_registry.get(&qualified_name) {
                                typestate.stack.push(funcsign.call(
                                    &method_args,
                                    &kwargs,
                                    listener.clone(),
                                )?);
                                if let (Some(def_span), Some(def_path), Some(def_unique_id)) = (
                                    funcsign.get_span(),
                                    funcsign.get_path(),
                                    funcsign.get_unique_id(),
                                ) {
                                    listener.on_function_call(
                                        identifier_span,
                                        &def_span,
                                        &def_path,
                                        &def_unique_id,
                                    );
                                }
                            } else {
                                typestate.stack.push(Type::Any { hard: false });
                            }
                            continue;
                        }

                        if self_type.is_any() {
                            typestate.stack.push(self_type);
                            continue;
                        }

                        let function = self_type.get_attribute(name, listener.clone())?;

                        if function.is_any() {
                            typestate.stack.push(function);
                            continue;
                        }

                        let result = match function.call(&method_args, &kwargs, listener.clone()) {
                            Ok(rv) => {
                                if let Type::Object(funcsign) = &rv {
                                    if let (Some(def_span), Some(def_path), Some(def_unique_id)) = (
                                        funcsign.get_span(),
                                        funcsign.get_path(),
                                        funcsign.get_unique_id(),
                                    ) {
                                        listener.on_function_call(
                                            identifier_span,
                                            &def_span,
                                            &def_path,
                                            &def_unique_id,
                                        );
                                    }
                                }
                                rv
                            }
                            Err(e) => {
                                listener
                                    .warn(&format!("Method call failed '{self_type}.{name}': {e}"));
                                Type::Any { hard: false }
                            }
                        };

                        typestate.stack.push(result);
                    } else {
                        // TODO: handle the case when arg_count is None
                        return Err(crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            format!(
                                "The first argument(self) of method call is missing at {span:?}"
                            ),
                        ));
                    }
                }
                Instruction::CallObject(arg_count, span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    let count = arg_count.unwrap_or(0);
                    if count > 0 {
                        // Pop (arg_count - 1) arguments
                        let (args, kwargs) = typestate.get_call_args(count - 1);
                        // Pop the last one as 'self'
                        let self_type = match typestate.stack.pop() {
                            Some(val) => val,
                            None => {
                                return Err(crate::Error::new(
                                    crate::error::ErrorKind::InvalidOperation,
                                    "Stack underflow on call method",
                                ))
                            }
                        };

                        if self_type.is_any() {
                            typestate.stack.push(self_type);
                            continue;
                        }

                        let result = self_type.call(&args, &kwargs, listener.clone())?;

                        typestate.stack.push(result);
                    } else {
                        // TODO: handle the case when arg_count is None
                        return Err(crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            format!(
                                "The first argument(self) of method call is missing at {span:?}"
                            ),
                        ));
                    }
                }
                Instruction::DupTop => {
                    // TYPECHECK: NO
                    // if no item on the stack, do nothing
                    if typestate.stack.is_empty() {
                        // DO NOTHING
                    } else {
                        typestate.stack.push(typestate.peek().clone());
                    }
                }
                Instruction::DiscardTop => {
                    // TYPECHECK: NO
                    typestate.stack.pop();
                }
                Instruction::FastSuper(_) => {
                    // TYPECHECK: NO
                    // Nothing to do with the stack
                }
                Instruction::FastRecurse(_) => {
                    // TYPECHECK: NO
                    // Nothing to do with the stack
                }
                #[cfg(feature = "multi_template")]
                Instruction::LoadBlocks(span) => {
                    // TYPECHECK: YES
                    listener.set_span(span);
                    // the operand must be a string
                    let a = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on load blocks",
                            ))
                        }
                    };

                    if !a.is_subtype_of(&Type::String(None).into()) {
                        listener.warn(&format!("Type mismatch for block name: type = {a}"));
                    }
                    // LoadBlocks does not change the stack, it just loads blocks
                }
                #[cfg(feature = "multi_template")]
                Instruction::Include(_ignore_missing, _span) => {
                    // TYPECHECK: NO
                    let _item_type = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on include",
                            ))
                        }
                    };
                }
                #[cfg(feature = "multi_template")]
                Instruction::ExportLocals => {
                    // TYPECHECK: NO
                    typestate.stack.push(Type::Any { hard: false });
                }
                #[cfg(feature = "macros")]
                Instruction::BuildMacro(name, _offset, _flags, span) => {
                    // TYPECHECK: NO?
                    listener.set_span(span);
                    // BuildMacro consume the parameter names in the stack
                    if typestate.stack.pop().is_none() {
                        return Err(crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            "Stack underflow on build macro",
                        ));
                    }
                    // BuildMacro consume the closure in the stack
                    if typestate.stack.pop().is_none() {
                        return Err(crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            "Stack underflow on build macro",
                        ));
                    }
                    // look up the function in the function registry
                    if let Some(macro_qualified_name) = macro_namespace_template_resolver(
                        &typecheck_resolved_context,
                        self.function_registry.clone(),
                        name,
                        attempts,
                    ) {
                        if let Some(function) = self.function_registry.get(&macro_qualified_name) {
                            typestate.stack.push(Type::Object(function.clone()));
                        } else {
                            listener.warn(&format!(
                                "Macro '{macro_qualified_name}' is not defined in the function registry."
                            ));
                            typestate.stack.push(Type::Any { hard: false });
                        }
                    } else if let Some(function) = self.function_registry.get(*name) {
                        typestate.stack.push(Type::Object(function.clone()));
                    } else if *name == "caller" {
                        typestate
                            .stack
                            .push(self.builtins.get("caller").unwrap().clone());
                    } else {
                        listener.warn(&format!(
                            "Function '{name}' is not defined in the function registry."
                        ));
                        typestate.stack.push(Type::Any { hard: false });
                    }
                }
                #[cfg(feature = "macros")]
                Instruction::Return { explicit, .. } => {
                    // TYPECHECK: NO
                    // do nothing instead of break because we want to cover all instructions
                    if *explicit {
                        // pop the stack as the return value
                        let rv_type = match typestate.stack.pop() {
                            Some(val) => val.inner,
                            None => {
                                return Err(crate::Error::new(
                                    crate::error::ErrorKind::InvalidOperation,
                                    "Stack underflow on return",
                                ))
                            }
                        };
                        typestate.rv_type = rv_type;
                    }
                }
                #[cfg(feature = "macros")]
                Instruction::Enclose(_name) => {
                    // TYPECHECK: NO
                    // Nothing to do with the stack
                }
                #[cfg(feature = "macros")]
                Instruction::GetClosure => {
                    // TYPECHECK: NO?
                    typestate.stack.push(Type::Any { hard: false });
                }
                Instruction::MacroStart(_line, _col, _index) => {
                    // TYPECHECK: NO
                    // Nothing to do with the stack
                }
                Instruction::MacroStop(_line, _col, _index) => {
                    // TYPECHECK: NO
                    // Nothing to do with the stack
                }
                Instruction::JinjaLayoutEvent(_kind, span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                }
                Instruction::JinjaLayoutEventIfLoopDidNotIterate(_kind, span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                }
                Instruction::JinjaLayoutLoopIterationStart(span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                }
                Instruction::MacroName(_name, span) => {
                    // TYPECHECK: NO
                    listener.set_span(span);
                }
                Instruction::TypeConstraint(type_constraint, _true_branch, span) => {
                    listener.set_span(span);
                    let name = &type_constraint.name;
                    match &type_constraint.operation {
                        TypeConstraintOperation::NotNull(is_true) => {
                            if *is_true {
                                if let Ok(type_) =
                                    typestate.locals.get(name, suppressed_listener.clone())
                                {
                                    if type_.is_optional() {
                                        let non_optional_type = type_.get_non_optional_type();
                                        typestate.locals.insert(
                                            name,
                                            non_optional_type,
                                            listener.clone(),
                                            None,
                                        )?;
                                    } else if type_.is_none() {
                                        typestate.locals.insert(
                                            name,
                                            Type::Any { hard: true },
                                            listener.clone(),
                                            None,
                                        )?;
                                    }
                                }
                            } else if let Ok(type_) =
                                typestate.locals.get(name, suppressed_listener.clone())
                            {
                                if type_.is_optional() {
                                    typestate.locals.insert(
                                        name,
                                        Type::None,
                                        listener.clone(),
                                        None,
                                    )?;
                                }
                            }
                        }
                        TypeConstraintOperation::Is(test_name, is_true) => {
                            let test_type = if test_name == "true" || test_name == "false" {
                                Type::Bool
                            } else {
                                Type::from_str(test_name)?
                            };
                            if !is_true {
                                if let Ok(type_) =
                                    typestate.locals.get(name, suppressed_listener.clone())
                                {
                                    typestate.locals.insert(
                                        name,
                                        type_.exclude(&test_type),
                                        listener.clone(),
                                        None,
                                    )?;
                                }
                            } else if let Ok(_type_) =
                                typestate.locals.get(name, suppressed_listener.clone())
                            {
                                typestate
                                    .locals
                                    .insert(name, test_type, listener.clone(), None)?;
                            }
                        }
                    }
                }
                Instruction::LoadType(value) => {
                    if let Some(object) = value.as_object() {
                        if let Some(type_) = object.downcast_ref::<Type>() {
                            let type_: TypeWithConstraint = type_.clone().into();
                            typestate.stack.push(type_);
                        } else {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Invalid type",
                            ));
                        }
                    } else {
                        return Err(crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            "Invalid type",
                        ));
                    }
                }
                Instruction::UnionType => {
                    let a = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on union type",
                            ));
                        }
                    };
                    let b = match typestate.stack.pop() {
                        Some(val) => val,
                        None => {
                            return Err(crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Stack underflow on union type",
                            ));
                        }
                    };
                    let union_type = a.union(&b);
                    typestate.stack.push(union_type);
                }
            }
            // println!(
            //     "After instruction {:?}, locals_temp_relation: {:?}",
            //     inst,
            //     typestate
            //         .locals
            //         .get("temp_relation", suppressed_listener.clone())
            // );
        }
        Ok(typestate)
    }

    /// Merges the source typecheck state into the destination state at the merge point.
    fn merge_into(
        dst: &mut TypecheckState,
        src: &TypecheckState,
        visited: bool,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> bool {
        let mut changed = false;

        let min_len = dst.stack.len().min(src.stack.len());
        dst.stack.truncate(min_len);

        if dst.cur_loop_obj_type != src.cur_loop_obj_type {
            dst.cur_loop_obj_type = match (&dst.cur_loop_obj_type, &src.cur_loop_obj_type) {
                (Some(a), Some(b)) => {
                    if a.is_subtype_of(b) {
                        Some(b.clone())
                    } else if b.is_subtype_of(a) {
                        Some(a.clone())
                    } else {
                        Some(Type::Any { hard: false })
                    }
                }
                (None, Some(t)) => Some(t.clone()),
                (Some(t), None) => Some(t.clone()),
                (None, None) => None,
            };
            changed = false;
        }

        for i in 0..min_len {
            let src_type = src.stack.get(i).unwrap().clone();
            let dst_type = &mut dst.stack.0[i];

            let dst_before = dst_type.inner.clone();
            let union_type = dst_type.clone().union(&src_type);
            if union_type.inner != dst_before {
                *dst_type = union_type.clone();
                changed = true;
            }
        }

        // Union all keys from both locals
        let all_keys: std::collections::HashSet<_> = dst
            .locals
            .keys()
            .chain(src.locals.keys())
            .cloned()
            .collect();

        for name in all_keys {
            dst.locals
                .locals_definitions_location
                .entry(name.clone())
                .or_default()
                .extend(
                    src.locals
                        .locals_definitions_location
                        .get(&name)
                        .cloned()
                        .unwrap_or_default(),
                );
            // remove duplicated locations
            dst.locals
                .locals_definitions_location
                .get_mut(&name)
                .unwrap()
                .sort_unstable();
            dst.locals
                .locals_definitions_location
                .get_mut(&name)
                .unwrap()
                .dedup();
            match (dst.locals.get_mut(&name), src.locals.get_ref(&name)) {
                // Both branches define the variable — union their types.
                (Some(dst_type), Some(src_type)) => {
                    let union_type = dst_type.union(src_type);
                    if union_type.inner != dst_type.inner {
                        *dst_type = union_type;
                        changed = true;
                    }
                }
                // Variable in dst (one branch) but not in src (other branch):
                // mark as single-branch-defined so Lookup warns later.
                // Note: no `!visited` guard here — at this point the join
                // block is already visited (set when the first predecessor
                // arrived via first_merge), so a visited check would always
                // skip this.
                (Some(_), None) => {
                    dst.single_branch_definition_vars.insert(name.clone());
                }
                // Variable in src (one branch) but not in dst (other branch):
                // mark as single-branch-defined and insert as Any.
                (None, Some(_src_type)) => {
                    if !visited {
                        dst.single_branch_definition_vars.insert(name.clone());
                    }
                    dst.locals
                        .insert(
                            name.clone(),
                            Type::Any { hard: true },
                            listener.clone(),
                            None,
                        )
                        .unwrap();
                    changed = true;
                }
                // Neither branch defines the variable — nothing to do.
                (None, None) => {}
            }
        }

        changed
    }
}

pub fn macro_namespace_template_resolver(
    typecheck_resolved_context: &BTreeMap<String, Value>,
    function_registry: Arc<BTreeMap<String, DynObject>>,
    search_name: &str,
    attempts: &mut Vec<String>,
) -> Option<String> {
    // Get necessary values from state
    let current_package_name = typecheck_resolved_context
        .get(TARGET_PACKAGE_NAME)
        .cloned()
        .unwrap_or_else(|| Value::from("dbt"));
    let current_package_name = current_package_name.as_str().unwrap();
    let root_package = typecheck_resolved_context
        .get(ROOT_PACKAGE_NAME)
        .cloned()
        .unwrap_or_else(|| Value::from("dbt"));
    let root_package = root_package.as_str().unwrap();
    // The published namespace is `dialect -> (macro_name -> package)`; select this
    // context's dialect so an unprefixed internal macro defined by more than one
    // loaded adapter resolves to the right package, matching dispatch.
    let dialect = typecheck_resolved_context
        .get(DIALECT)
        .and_then(|v| v.as_str())
        .unwrap_or("postgres")
        .to_string();
    let namespaces = typecheck_resolved_context
        .get(DBT_AND_ADAPTERS_NAMESPACE)
        .cloned()
        .unwrap_or_default();
    let namespaces = namespaces
        .as_object()
        .and_then(|o| o.downcast_ref::<ValueMap>().cloned())
        .unwrap_or_default();
    let dbt_and_adapters = dbt_and_adapters_namespace_for(&namespaces, &dialect);

    // 1. Local namespace (current package)
    let template_name = format!("{current_package_name}.{search_name}");
    attempts.push(template_name.clone());
    if function_registry.contains_key(&template_name) {
        return Some(template_name);
    }

    // 2. Root package namespace
    let template_name = format!("{root_package}.{search_name}");
    attempts.push(template_name.clone());
    if function_registry.contains_key(&template_name) {
        return Some(template_name);
    }

    // 3. Internal packages
    let search_name_value = Value::from(search_name);
    if let Some(pkg) = dbt_and_adapters.get(&search_name_value) {
        let template_name = format!("{pkg}.{search_name}");
        attempts.push(template_name.clone());
        if function_registry.contains_key(&template_name) {
            return Some(template_name);
        }
    }

    // No template found
    None
}
