use std::collections::BTreeMap;
use std::mem;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) use macro_object::Macro;

use crate::compiler::instructions::{
    Instruction, Instructions, LOOP_FLAG_RECURSIVE, LOOP_FLAG_WITH_LOOP_VAR, MAX_LOCALS,
};
use crate::constants::{CURRENT_PATH, CURRENT_SPAN};
use crate::dispatch_object::DispatchObject;
use crate::environment::Environment;
use crate::error::{Error, ErrorKind};
use crate::listener::RenderingEventListener;
use crate::machinery::Span;
use crate::output::{CaptureMode, Output};
use crate::utils::{untrusted_size_hint, AutoEscape};
use crate::value::introspective::IntrospectiveValue;
use crate::value::mutable_map::MutableMap;
use crate::value::namespace_object::Namespace;
use crate::value::Object;
use crate::value::{
    mutable_map, mutable_vec, ops, value_map_with_capacity, value_optimization, Kwargs, ObjectRepr,
    Value, ValueMap,
};
use crate::vm::context::{LoopState, Stack};
use crate::vm::loop_object::Loop;
use crate::vm::state::BlockStack;

#[cfg(feature = "macros")]
use crate::vm::closure_object::Closure;

pub(crate) use crate::vm::context::{Context, Frame};
pub use crate::vm::state::State;
use crate::CodeLocation;

#[cfg(feature = "macros")]
mod closure_object;
mod context;
#[cfg(feature = "fuel")]
mod fuel;
pub mod listeners;
mod loop_object;
#[cfg(feature = "macros")]
mod macro_object;
mod state;
pub mod typemeta;

// the cost of a single include against the stack limit.
#[cfg(feature = "multi_template")]
pub(crate) const INCLUDE_RECURSION_COST: usize = 10;

// the cost of a single macro call against the stack limit.
#[cfg(feature = "macros")]
pub(crate) const MACRO_RECURSION_COST: usize = 4;

// Marker text substituted in place of a value a listener overrode via
// `RenderingEventListener::override_value` when it's emitted (see
// `Instruction::Emit`). Keep in sync with `sdf_linter::skeleton::HOLE_MARKER`
// (crates/sdf-linter/src/skeleton.rs) so Turbo and listener-driven overrides
// produce visually identical hole markers.
pub(crate) const VALUE_OVERRIDE_MARKER: &str = "{{}}";

/// Helps to evaluate something.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
pub struct Vm<'env> {
    env: &'env Environment<'env>,
}

pub(crate) fn prepare_blocks<'env, 'template>(
    blocks: &'template BTreeMap<&'env str, Instructions<'env>>,
) -> BTreeMap<&'env str, BlockStack<'template, 'env>> {
    blocks
        .iter()
        .map(|(name, instr)| (*name, BlockStack::new(instr)))
        .collect()
}

fn get_or_lookup_local<T, F>(vec: &mut [Option<T>], local_id: u8, f: F) -> Option<T>
where
    T: Copy,
    F: FnOnce() -> Option<T>,
{
    if local_id == !0 {
        f()
    } else if let Some(Some(rv)) = vec.get(local_id as usize) {
        Some(*rv)
    } else {
        let val = some!(f());
        vec[local_id as usize] = Some(val);
        Some(val)
    }
}

fn span_with_current_offset(span: &Span, offset: &Span) -> Span {
    Span {
        start_line: span.start_line + offset.start_line - 1,
        start_col: span.start_col
            + if offset.start_line == 1 {
                offset.start_col.saturating_sub(1)
            } else {
                0
            },
        start_offset: span.start_offset + offset.start_offset,
        end_line: span.end_line + offset.start_line - 1,
        end_col: span.end_col
            + if span.end_line == 1 {
                offset.start_col.saturating_sub(1)
            } else {
                0
            },
        end_offset: span.end_offset + offset.end_offset,
    }
}

/// Wrapper for return values originating from within a `{% call %}` block.
///
/// When `return()` is called inside a `{% call %}` block (i.e., within a `caller()` invocation),
/// the return value must propagate up **2 levels** of the call stack:
///
/// 1. **Level 1 (caller block)**: The `caller()` function executes the call block body.
///    When a `return()` is encountered, the value is wrapped in `CallerReturn` and returned
///    from the `caller()` call.
///
/// 2. **Level 2 (enclosing macro)**: The macro that invoked `caller()` receives the
///    `CallerReturn` wrapper. It must unwrap the value and treat it as an explicit return,
///    causing the macro itself to return that value to its caller.
///
/// Without this 2-level propagation, the return value would be consumed by the inner
/// `caller()` invocation but not propagate out of the enclosing macro.
///
/// Example:
/// ```jinja
/// {%- macro my_statement() -%}
///     {%- set compiled_code = caller() -%}
/// {%- endmacro -%}
/// {%- macro outer() -%}
///   {%- call my_statement() -%}
///     {{- return('value') -}}  {# This return must propagate out of outer() #}
///   {%- endcall -%}
/// {%- endmacro -%}
/// ```
#[derive(Clone, Debug)]
struct CallerReturn {
    value: Value,
}

impl Object for CallerReturn {}

impl CallerReturn {
    pub fn new(value: Value) -> Self {
        CallerReturn { value }
    }
}

/// Wrapper function for calling a function.
/// This wrapper tells listeners that the function is being entered and exited.
/// The reason we need to track function entry and exit is because of malicious return
/// malicious return causes unbalanced MacroStart and MacroStop
/// e.g. {% if condition %} {{ return(1) + 1 }} {% endif %}
/// The rendering flow creates 2 MacroStart and encounters return. If we don't track function entry and exit,
/// we will not know when to pop the MacroStart from the stack.
fn call_wrapper(
    listeners: &[Rc<dyn RenderingEventListener>],
    function_call: impl FnOnce() -> Result<Value, Error>,
) -> Result<Value, Error> {
    listeners.iter().for_each(|listener| {
        listener.on_function_start();
    });
    let rv = function_call();
    listeners.iter().for_each(|listener| {
        listener.on_function_end();
    });
    rv
}

/// Rewrites an "unknown method" error so it names the undefined receiver.
///
/// `Value::call_method` only has the receiver's *value*, so its fallback detail can
/// say no more than `undefined has no method named meta_get` — which reads as though
/// the method were unsupported, when the real problem is that the receiver was never
/// defined.
///
/// Two sources for the name, preferred in this order:
///
/// 1. The value's own recorded origin ([`Value::undefined_name`]), set where the
///    failed lookup happened. This is the identifier the author wrote, and it holds
///    even when the value has since been passed into a macro and rebound to a
///    parameter — which is the name dbt Core reports.
/// 2. `receiver_name`, the identifier as written at this call site, when the receiver
///    is a plain variable (see `Instruction::CallMethod`). Covers undefined values
///    that did not come from a variable lookup.
///
/// Still best-effort: with neither available, the original detail is left unchanged.
fn name_undefined_receiver(
    mut err: Error,
    receiver: &Value,
    receiver_name: Option<&str>,
    method: &str,
) -> Error {
    if err.kind() == ErrorKind::UnknownMethod && receiver.is_undefined() {
        if let Some(name) = receiver.undefined_name().or(receiver_name) {
            err.set_detail(format!(
                "`{name}` is undefined, so it has no method named `{method}`"
            ));
        }
    }
    err
}

impl<'env> Vm<'env> {
    /// Creates a new VM.
    pub fn new(env: &'env Environment<'env>) -> Vm<'env> {
        Vm { env }
    }

    /// Evaluates the given inputs.
    ///
    /// It returns both the last value left on the stack as well as the state
    /// at the end of the evaluation.
    #[allow(clippy::too_many_arguments)]
    pub fn eval<'template>(
        &self,
        instructions: &'template Instructions<'env>,
        root: Value,
        blocks: &'template BTreeMap<&'env str, Instructions<'env>>,
        auto_escape: AutoEscape,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<(Value, State<'template, 'env>), Error> {
        self.eval_with_outer_stack_depth(instructions, root, blocks, auto_escape, listeners, 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn eval_with_outer_stack_depth<'template>(
        &self,
        instructions: &'template Instructions<'env>,
        root: Value,
        blocks: &'template BTreeMap<&'env str, Instructions<'env>>,
        auto_escape: AutoEscape,
        listeners: &[Rc<dyn RenderingEventListener>],
        outer_stack_depth: usize,
    ) -> Result<(Value, State<'template, 'env>), Error> {
        let _guard = value_optimization();

        let ctx = ok!(Context::new_with_frame_and_stack_depth(
            ok!(Frame::new_checked(root.clone())),
            self.env.recursion_limit(),
            root.get_attr_fast(CURRENT_PATH)
                .map_or_else(PathBuf::new, |value| deserialize_path(&value)),
            root.get_attr_fast(CURRENT_SPAN)
                .map_or_else(Span::default, |value| deserialize_span(&value)),
            outer_stack_depth,
        ));

        let mut state = State::new(
            self.env,
            ctx,
            auto_escape,
            instructions,
            prepare_blocks(blocks),
        );
        self.eval_state(&mut state, listeners).map(|x| (x, state))
    }

    /// Evaluate a macro in a state.
    #[cfg(feature = "macros")]
    #[allow(clippy::too_many_arguments)]
    pub fn eval_macro(
        &self,
        instructions: &Instructions<'env>,
        pc: usize,
        closure: Value,
        context_base: Value,
        caller: Option<Value>,
        varargs: Vec<Value>,
        kwargs: MutableMap,
        state: &State,
        args: Vec<Value>,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let path = context_base
            .get_attr_fast(CURRENT_PATH)
            .map_or_else(PathBuf::new, |value| deserialize_path(&value));
        let span = context_base
            .get_attr_fast(CURRENT_SPAN)
            .map_or_else(Span::default, |value| deserialize_span(&value));
        let mut ctx = Context::new_with_frame(
            Frame::new(context_base),
            self.env.recursion_limit(),
            path,
            span,
        );
        ok!(ctx.push_frame(Frame::new(closure)));
        if let Some(caller) = caller {
            ctx.store("caller", caller);
        }
        ctx.store("varargs", Value::from(varargs));
        // kwargs is already a MutableMap, supporting .pop() and other mutating methods
        ctx.store("kwargs", Value::from_object(kwargs));

        ok!(ctx.incr_depth(state.ctx.depth() + MACRO_RECURSION_COST));
        // Nested macro calls should not use listener's location to avoid sync issues
        self.do_eval(
            &mut State {
                env: self.env,
                ctx,
                current_block: None,
                auto_escape: state.auto_escape(),
                instructions,
                blocks: BTreeMap::default(),
                loaded_templates: Default::default(),
                #[cfg(feature = "macros")]
                id: state.id,
                #[cfg(feature = "macros")]
                macros: state.macros.clone(),
                #[cfg(feature = "macros")]
                closure_tracker: state.closure_tracker.clone(),
                #[cfg(feature = "fuel")]
                fuel_tracker: state.fuel_tracker.clone(),

                pc,
                pending_call_site: None,
            },
            Stack::from(args),
            pc,
            listeners,
            false,
        )
    }

    /// This is the actual evaluation loop that works with a specific context.
    #[inline(always)]
    fn eval_state(
        &self,
        state: &mut State<'_, 'env>,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        // Top-level eval: use listener's location for macro span tracking
        self.do_eval(state, Stack::default(), 0, listeners, true)
    }

    /// Performs the actual evaluation, optionally with stack growth functionality.
    #[allow(clippy::too_many_arguments)]
    fn do_eval(
        &self,
        state: &mut State<'_, 'env>,
        stack: Stack,
        pc: usize,
        listeners: &[Rc<dyn RenderingEventListener>],
        use_listener_location: bool,
    ) -> Result<Value, Error> {
        #[cfg(feature = "stacker")]
        {
            stacker::maybe_grow(32 * 1024, 1024 * 1024, || {
                self.eval_impl(state, stack, pc, listeners, use_listener_location)
            })
        }
        #[cfg(not(feature = "stacker"))]
        {
            self.eval_impl(state, stack, pc, listeners, use_listener_location)
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn eval_impl(
        &self,
        state: &mut State<'_, 'env>,
        mut stack: Stack,
        mut pc: usize,
        listeners: &[Rc<dyn RenderingEventListener>],
        use_listener_location: bool,
    ) -> Result<Value, Error> {
        let mut rv = String::new();

        // Only use listener's output tracker for top-level render (macro span tracking).
        // Nested macro calls must use their own tracker to avoid sync issues.
        let mut output_tracker = 'tracker: {
            if use_listener_location {
                for listener in listeners {
                    if let Some(tracker) = listener.create_output_tracker(&mut rv) {
                        break 'tracker tracker;
                    }
                }
            }
            crate::OutputTracker::new(&mut rv)
        };
        // Cloned before `out` borrows `output_tracker` mutably so the current
        // output position can still be read (via the shared `RefCell`s) when
        // substituting a value-override marker (see `VALUE_OVERRIDE_MARKER`).
        let output_location = output_tracker.location.clone();
        let mut out = Output::with_write(&mut output_tracker);

        let initial_auto_escape = state.auto_escape;
        let undefined_behavior = state.undefined_behavior();
        let mut auto_escape_stack = vec![];
        let mut next_loop_recursion_jump = None;
        let mut loaded_filters = [None; MAX_LOCALS];
        let mut loaded_tests = [None; MAX_LOCALS];
        let mut current_macro_name: Option<String> = None;
        let mut is_caller_return = false;
        let mut is_explicit_return = false;
        let override_listener = crate::listener::find_override_listener(listeners);

        // If we are extending we are holding the instructions of the target parent
        // template here.  This is used to detect multiple extends and the evaluation
        // uses these instructions when it makes it to the end of the instructions.
        #[cfg(feature = "multi_template")]
        let mut parent_instructions = None;

        macro_rules! recurse_loop {
            ($capture:expr, $span:expr) => {{
                let jump_target = self
                    .prepare_loop_recursion(state)
                    .map_err(|e| state.with_span_error(e, &$span))?;
                // the way this works is that we remember the next instruction
                // as loop exit jump target.  Whenever a loop is pushed, it
                // memorizes the value in `next_loop_iteration_jump` to jump
                // to.
                next_loop_recursion_jump = Some((pc + 1, $capture));
                if $capture {
                    out.begin_capture(CaptureMode::Capture);
                }
                pc = jump_target;
                continue;
            }};
        }

        let root_package_name = self.env.get_root_package_name();
        let template_registry = self.env.get_macro_template_registry();

        #[allow(clippy::while_let_loop)]
        loop {
            let instr = match state.instructions.get(pc) {
                Some(instr) => instr,
                #[cfg(not(feature = "multi_template"))]
                None => break,
                #[cfg(feature = "multi_template")]
                None => {
                    // when an extends statement appears in a template, when we hit the
                    // last instruction we need to check if parent instructions were
                    // stashed away (which means we found an extends tag which invoked
                    // `LoadBlocks`).  If we do find instructions, we reset back to 0
                    // from the new instructions.
                    state.instructions = match parent_instructions.take() {
                        Some(instr) => instr,
                        None => break,
                    };
                    out.end_capture(AutoEscape::None);
                    pc = 0;
                    // because we swap out the instructions we also need to unload all
                    // the filters and tests to ensure that we are not accidentally
                    // reusing the local_ids for completely different filters.
                    loaded_filters = [None; MAX_LOCALS];
                    loaded_tests = [None; MAX_LOCALS];
                    continue;
                }
            };
            state.pc = pc;

            // If a listener wants to override either operand of a
            // binary/comparison operator (see
            // `RenderingEventListener::override_value`), the real operator is
            // never evaluated -- the overridden operand is propagated as the
            // result unchanged, so an override survives arithmetic, string
            // concatenation, and comparisons alike.
            macro_rules! func_binop {
                ($method:ident, $obj_method:expr, $span:expr) => {{
                    let b = stack.pop();
                    let a = stack.pop();
                    let overridden = override_listener
                        .and_then(|l| l.override_value(&a).or_else(|| l.override_value(&b)));
                    stack.push(match overridden {
                        Some(v) => v,
                        None => match ops::$method(&a, &b) {
                            Ok(rv) => rv,
                            Err(e) if e.kind() == ErrorKind::InvalidOperation => {
                                match call_wrapper(listeners, || {
                                    a.call_method(state, $obj_method, &[b], listeners)
                                }) {
                                    Ok(rv) => rv,
                                    Err(e2) if e2.kind() == ErrorKind::UnknownMethod => {
                                        return Err(state.with_span_error(e, &$span));
                                    }
                                    Err(e2) => return Err(state.with_span_error(e2, &$span)),
                                }
                            }
                            Err(e) => return Err(state.with_span_error(e, &$span)),
                        },
                    });
                }};
            }

            macro_rules! op_binop {
                ($op:tt, $obj_method:expr, $span:expr) => {{
                    let b = stack.pop();
                    let a = stack.pop();
                    let overridden = override_listener
                        .and_then(|l| l.override_value(&a).or_else(|| l.override_value(&b)));
                    let rv = match overridden {
                        Some(v) => v,
                        None => {
                            let naive_result = Value::from(a $op b);
                            match call_wrapper(listeners, || {
                                a.call_method(state, $obj_method, &[b], listeners)
                            }) {
                                Ok(rv) => rv,
                                Err(e) if e.kind() == ErrorKind::UnknownMethod => naive_result,
                                Err(e) => return Err(state.with_span_error(e, &$span))
                            }
                        }
                    };
                    stack.push(rv);
                }};
            }

            // if the fuel consumption feature is enabled, track the fuel
            // consumption here.
            #[cfg(feature = "fuel")]
            if let Some(ref tracker) = state.fuel_tracker {
                tracker.track(instr)?;
            }

            // dbg!(&instr);
            match instr {
                Instruction::Swap => {
                    let a = stack.pop();
                    let b = stack.pop();
                    stack.push(a);
                    stack.push(b);
                }
                Instruction::EmitRaw(val, span) => {
                    if current_macro_name.is_none() && !out.is_capturing() {
                        let source_span = span_with_current_offset(span, &state.ctx.current_span);
                        listeners.iter().for_each(|listener| {
                            listener.on_raw_emit(val, &source_span);
                        });
                    }
                    // this only produces a format error, no need to attach
                    // location information.
                    out.write_str(val).map_err(|e| {
                        let e: Error = e.into();
                        state.with_span_error(e, span)
                    })?;
                }
                Instruction::Emit(span) => {
                    let source_span = if current_macro_name.is_none() && !out.is_capturing() {
                        Some(span_with_current_offset(span, &state.ctx.current_span))
                    } else {
                        None
                    };
                    if let Some(source_span) = &source_span {
                        listeners
                            .iter()
                            .for_each(|listener| listener.on_emit_start(source_span));
                    }
                    let value = stack.pop();
                    if override_listener
                        .and_then(|l| l.override_value(&value))
                        .is_some()
                    {
                        let expanded_start = (
                            output_location.line(),
                            output_location.col(),
                            output_location.index(),
                        );
                        out.write_str(VALUE_OVERRIDE_MARKER).map_err(|e| {
                            let e: Error = e.into();
                            state.with_span_error(e, span)
                        })?;
                        if let Some(source_span) = &source_span {
                            let expanded_span = Span {
                                start_line: expanded_start.0,
                                start_col: expanded_start.1,
                                start_offset: expanded_start.2,
                                end_line: output_location.line(),
                                end_col: output_location.col(),
                                end_offset: output_location.index(),
                            };
                            listeners.iter().for_each(|listener| {
                                listener.on_value_override(source_span, &expanded_span)
                            });
                        }
                    } else {
                        self.env
                            .format(&value, state, &mut out)
                            .map_err(|e| state.with_span_error(e, span))?;
                    }
                    if let Some(source_span) = &source_span {
                        listeners
                            .iter()
                            .for_each(|listener| listener.on_emit_end(source_span));
                    }
                }
                Instruction::StoreLocal(name, _) => {
                    let value = stack.pop();
                    // dbg!((name, &value));
                    // check if the value is a Map, if so, convert it to MutableMap
                    if let Some(value_map) = value.downcast_object_ref::<ValueMap>() {
                        let mutable_map = MutableMap::from(value_map.clone());
                        state.ctx.store(name, Value::from_object(mutable_map));
                    } else {
                        state.ctx.store(name, value);
                    }
                }
                Instruction::Lookup(name, span) => {
                    if *name == "this" {
                        listeners.iter().for_each(|listener| {
                            listener.on_this_reference(
                                span.start_line,
                                span.start_col,
                                span.start_offset,
                                span.end_line,
                                span.end_col,
                                span.end_offset,
                            );
                        });
                    }
                    // Remember which identifier failed to resolve. The name travels
                    // with the value, so an error raised after it has been passed
                    // somewhere else — into a macro, say — can still report the
                    // variable the author actually wrote.
                    stack.push(
                        state
                            .lookup(name, listeners)
                            .unwrap_or_else(|| Value::undefined_named(*name)),
                    )
                }
                Instruction::GetAttr(name, span) => {
                    let a = stack.pop();
                    // For Map values, reserved dict method names (`items` /
                    // `keys` / `values` / `get`) must resolve to a bound method
                    // — not the user's value at that key. Python parity:
                    // `d.items` is the class attribute, `d['items']` is the
                    // user value.
                    let value = crate::value::bound_dict_method_for_attr(&a, name)
                        .or_else(|| a.get_attr_fast(name))
                        .or_else(|| a.as_object()?.get_property(state, name, listeners).ok());
                    stack.push(match value {
                        Some(value) => value
                            .validate()
                            .map_err(|e| state.with_span_error(e, span))?,
                        None => undefined_behavior
                            .handle_undefined(a.is_undefined())
                            .map_err(|e| state.with_span_error(e, span))?,
                    });
                }
                Instruction::SetAttr(name, span) => {
                    let b = stack.pop();
                    let a = stack.pop();
                    if let Some(ns) = b.downcast_object_ref::<Namespace>() {
                        ns.set_value(name, a);
                    } else {
                        return Err(state.with_span_error(
                            Error::new(
                                ErrorKind::InvalidOperation,
                                format!("can only assign to namespaces, not {}", b.kind()),
                            ),
                            span,
                        ));
                    }
                }
                Instruction::GetItem(span) => {
                    let a = stack.pop();
                    let b = stack.pop();
                    // Fall back to get_property for string keys, mirroring GetAttr behavior
                    let value = b.get_item_opt(&a).or_else(|| {
                        let key = a.as_str()?;
                        b.as_object()?.get_property(state, key, listeners).ok()
                    });
                    stack.push(match value {
                        Some(value) => value
                            .validate()
                            .map_err(|e| state.with_span_error(e, span))?,
                        None => undefined_behavior
                            .handle_undefined(b.is_undefined())
                            .map_err(|e| state.with_span_error(e, span))?,
                    });
                }
                Instruction::Slice(span) => {
                    let step = stack.pop();
                    let stop = stack.pop();
                    let b = stack.pop();
                    let a = stack.pop();
                    // `ops::slice` only understands `ObjectRepr::Seq`/`Iterable`
                    // (or scalars it knows how to index); a tainted value's
                    // fabricated inner value is usually neither (most stub
                    // methods return `ObjectRepr::Plain` placeholders), which
                    // would otherwise fail with "cannot be sliced" instead of
                    // propagating the taint like every other operator here.
                    let overridden = override_listener.and_then(|l| {
                        l.override_value(&a)
                            .or_else(|| l.override_value(&b))
                            .or_else(|| l.override_value(&stop))
                            .or_else(|| l.override_value(&step))
                    });
                    stack.push(match overridden {
                        Some(v) => v,
                        None => ops::slice(a, b, stop, step)
                            .map_err(|e| state.with_span_error(e, span))?,
                    });
                }
                Instruction::LoadConst(value) => {
                    stack.push(value.clone());
                }
                Instruction::BuildMap(pair_count, _) => {
                    let mut map = value_map_with_capacity(*pair_count);
                    stack.reverse_top(*pair_count * 2);
                    for _ in 0..*pair_count {
                        let key = stack.pop();
                        let value = stack.pop();
                        map.insert(key, value);
                    }
                    let map = mutable_map::MutableMap::from(map);
                    stack.push(Value::from_object(map))
                }
                Instruction::BuildKwargs(pair_count) => {
                    let mut map = value_map_with_capacity(*pair_count);
                    stack.reverse_top(*pair_count * 2);
                    for _ in 0..*pair_count {
                        let key = stack.pop();
                        let value = stack.pop();
                        map.insert(key, value);
                    }
                    stack.push(Kwargs::wrap(map))
                }
                Instruction::MergeKwargs(count) => {
                    let mut kwargs_sources = Vec::new();
                    for _ in 0..*count {
                        kwargs_sources.push(stack.pop());
                    }
                    kwargs_sources.reverse();
                    let values: &[Value] = &kwargs_sources;
                    let mut rv = ValueMap::new();
                    for value in values {
                        self.env.undefined_behavior().assert_iterable(value)?;
                        let iter = value
                            .as_object()
                            .filter(|x| x.repr() == ObjectRepr::Map)
                            .and_then(|x| x.try_iter_pairs())
                            .ok_or_else(|| {
                                Error::new(
                                    ErrorKind::InvalidOperation,
                                    format!(
                                        "attempted to apply keyword arguments from non map (got {})",
                                        value.kind()
                                    ),
                                )
                            })?;
                        for (key, value) in iter {
                            rv.insert(key, value);
                        }
                    }
                    stack.push(Kwargs::wrap(rv));
                }
                Instruction::BuildList(n, _span) => {
                    let count = n.unwrap_or_else(|| stack.pop().try_into().unwrap());
                    let mut v = Vec::with_capacity(untrusted_size_hint(count));
                    for _ in 0..count {
                        v.push(stack.pop());
                    }
                    v.reverse();
                    let v = mutable_vec::MutableVec::from(v);
                    stack.push(Value::from_object(v))
                }
                Instruction::BuildTuple(count, _span) => {
                    let count = count.unwrap_or_else(|| stack.pop().try_into().unwrap());
                    let mut v = Vec::with_capacity(untrusted_size_hint(count));
                    for _ in 0..count {
                        v.push(stack.pop());
                    }
                    v.reverse();
                    stack.push(Value::from_object(v))
                }
                Instruction::UnpackList(count, span) => {
                    self.unpack_list(&mut stack, *count)
                        .map_err(|e| state.with_span_error(e, span))?;
                }
                Instruction::UnpackLists(count, span) => {
                    let mut lists = Vec::new();
                    for _ in 0..*count {
                        lists.push(stack.pop());
                    }
                    let mut len = 0;
                    for list in lists.into_iter().rev() {
                        for item in list
                            .try_iter()
                            .map_err(|e| state.with_span_error(e, span))?
                        {
                            stack.push(item);
                            len += 1;
                        }
                    }
                    stack.push(Value::from(len));
                }
                Instruction::Add(span) => {
                    func_binop!(add, "__add__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Sub(span) => {
                    func_binop!(sub, "__sub__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Mul(span) => {
                    func_binop!(mul, "__mul__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Div(span) => func_binop!(
                    div,
                    "__truediv__",
                    span.with_offset(&state.ctx.current_span)
                ),
                Instruction::IntDiv(span) => func_binop!(
                    int_div,
                    "__floordiv__",
                    span.with_offset(&state.ctx.current_span)
                ),
                Instruction::Rem(span) => {
                    func_binop!(rem, "__mod__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Pow(span) => {
                    func_binop!(pow, "__pow__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Eq(span) => {
                    op_binop!(==, "__eq__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Ne(span) => {
                    op_binop!(!=, "__ne__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Gt(span) => {
                    op_binop!(>, "__gt__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Gte(span) => {
                    op_binop!(>=, "__ge__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Lt(span) => {
                    op_binop!(<, "__lt__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Lte(span) => {
                    op_binop!(<=, "__le__", span.with_offset(&state.ctx.current_span))
                }
                Instruction::Not(_span) => {
                    let a = stack.pop();
                    let overridden = override_listener.and_then(|l| l.override_value(&a));
                    stack.push(match overridden {
                        Some(v) => v,
                        None => Value::from(!a.is_true()),
                    });
                }
                Instruction::StringConcat(_span) => {
                    let a = stack.pop();
                    let b = stack.pop();
                    let overridden = override_listener
                        .and_then(|l| l.override_value(&a).or_else(|| l.override_value(&b)));
                    stack.push(match overridden {
                        Some(v) => v,
                        None => ops::string_concat(b, &a),
                    });
                }
                Instruction::In(span) => {
                    let a = stack.pop();
                    let b = stack.pop();
                    let overridden = override_listener
                        .and_then(|l| l.override_value(&a).or_else(|| l.override_value(&b)));
                    match overridden {
                        Some(v) => stack.push(v),
                        None => {
                            // the in-operator can fail if the value is undefined and
                            // we are in strict mode.
                            state.undefined_behavior().assert_iterable(&a)?;
                            stack.push(
                                ops::contains(&a, &b)
                                    .map_err(|e| state.with_span_error(e, span))?,
                            );
                        }
                    }
                }
                Instruction::Neg(span) => {
                    let a = stack.pop();
                    let overridden = override_listener.and_then(|l| l.override_value(&a));
                    stack.push(match overridden {
                        Some(v) => v,
                        None => ops::neg(&a).map_err(|e| state.with_span_error(e, span))?,
                    });
                }
                Instruction::Pos(span) => {
                    let a = stack.pop();
                    let overridden = override_listener.and_then(|l| l.override_value(&a));
                    stack.push(match overridden {
                        Some(v) => v,
                        None => ops::pos(&a).map_err(|e| state.with_span_error(e, span))?,
                    });
                }
                Instruction::PushWith(span) => {
                    state
                        .ctx
                        .push_frame(Frame::default())
                        .map_err(|e| state.with_span_error(e, span))?;
                }
                Instruction::PopFrame => {
                    if let Some(mut loop_ctx) = state.ctx.pop_frame().current_loop {
                        if let Some((target, end_capture)) = loop_ctx.current_recursion_jump.take()
                        {
                            pc = target;
                            if end_capture {
                                stack.push(out.end_capture(state.auto_escape));
                            }
                            continue;
                        }
                    }
                }
                #[cfg(feature = "macros")]
                Instruction::IsUndefined => {
                    let a = stack.pop();
                    stack.push(Value::from(a.is_undefined()));
                }
                Instruction::PushLoop(flags, span) => {
                    let a = stack.pop();
                    self.push_loop(state, a, *flags, pc, next_loop_recursion_jump.take())
                        .map_err(|e| state.with_span_error(e, span))?;
                }
                Instruction::Iterate(jump_target, span) => {
                    let l = state.ctx.current_loop().unwrap();
                    l.object.idx.fetch_add(1, Ordering::Relaxed);

                    let next = {
                        #[cfg(feature = "adjacent_loop_items")]
                        {
                            let mut triple = l.object.value_triple.lock().unwrap();
                            triple.0 = triple.1.take();
                            triple.1 = triple.2.take();
                            triple.2 = l.iterator.next();
                            triple.1.clone()
                        }
                        #[cfg(not(feature = "adjacent_loop_items"))]
                        {
                            l.iterator.next()
                        }
                    };
                    match next {
                        Some(item) => {
                            // each iteration should start with clean local vars to match Python
                            state.ctx.current_locals_mut().clear();
                            stack.push(
                                item.validate()
                                    .map_err(|e| state.with_span_error(e, span))?,
                            );
                        }
                        None => {
                            pc = *jump_target;
                            continue;
                        }
                    };
                }
                Instruction::PushDidNotIterate => {
                    let l = state.ctx.current_loop().unwrap();
                    stack.push(Value::from(l.object.idx.load(Ordering::Relaxed) == 0));
                }
                Instruction::Jump(jump_target, _) => {
                    pc = *jump_target;
                    continue;
                }
                Instruction::JumpIfFalse(jump_target, span) => {
                    let a = stack.pop();
                    let take_then_branch =
                        match override_listener.and_then(|l| l.override_branch(&a)) {
                            Some(overridden) => overridden,
                            None => undefined_behavior
                                .is_true(&a)
                                .map_err(|e| state.with_span_error(e, span))?,
                        };
                    if !take_then_branch {
                        pc = *jump_target;
                        continue;
                    }
                }
                Instruction::JumpIfTrue(jump_target, span) => {
                    let a = stack.pop();
                    if undefined_behavior
                        .is_true(&a)
                        .map_err(|e| state.with_span_error(e, span))?
                    {
                        pc = *jump_target;
                        continue;
                    }
                }
                Instruction::JumpIfFalseOrPop(jump_target, span) => {
                    // `and`'s short-circuit point: on a tainted LHS, deciding
                    // "truthy" purely from the fabricated fake value (as plain
                    // `is_true` would) can short-circuit away a *real*,
                    // non-tainted RHS that would have deterministically made
                    // the whole expression false regardless of the LHS's real
                    // value (e.g. `is_incremental() and flags.WHICH != "lint"`
                    // during lint, where the RHS is always false). Consulting
                    // `override_branch` here mirrors `JumpIfFalse` below: when
                    // exploration wants to treat the tainted LHS as truthy, it
                    // must actually continue on to evaluate the RHS for real
                    // instead of blindly short-circuiting on the fake value.
                    let treat_as_true =
                        match override_listener.and_then(|l| l.override_branch(stack.peek())) {
                            Some(overridden) => overridden,
                            None => undefined_behavior
                                .is_true(stack.peek())
                                .map_err(|e| state.with_span_error(e, span))?,
                        };
                    if !treat_as_true {
                        pc = *jump_target;
                        continue;
                    } else {
                        stack.pop();
                    }
                }
                Instruction::JumpIfTrueOrPop(jump_target, span) => {
                    // `or`'s short-circuit point; see `JumpIfFalseOrPop` above
                    // for why this must also be taint/override-aware.
                    let treat_as_true =
                        match override_listener.and_then(|l| l.override_branch(stack.peek())) {
                            Some(overridden) => overridden,
                            None => undefined_behavior
                                .is_true(stack.peek())
                                .map_err(|e| state.with_span_error(e, span))?,
                        };
                    if treat_as_true {
                        pc = *jump_target;
                        continue;
                    } else {
                        stack.pop();
                    }
                }
                #[cfg(feature = "multi_template")]
                Instruction::CallBlock(name) => {
                    if parent_instructions.is_none() && !out.is_discarding() {
                        out.write_str(
                            call_wrapper(listeners, || self.call_block(name, state, listeners))?
                                .as_str()
                                .unwrap_or_default(),
                        )?;
                    }
                }
                Instruction::PushAutoEscape(span) => {
                    let a = stack.pop();
                    auto_escape_stack.push(state.auto_escape);
                    state.auto_escape = self
                        .derive_auto_escape(a, initial_auto_escape)
                        .map_err(|e| state.with_span_error(e, span))?;
                }
                Instruction::PopAutoEscape => {
                    state.auto_escape = auto_escape_stack.pop().unwrap();
                }
                Instruction::BeginCapture(mode) => {
                    out.begin_capture(*mode);
                }
                Instruction::EndCapture => {
                    stack.push(out.end_capture(state.auto_escape));
                }
                Instruction::ApplyFilter(name, arg_count, local_id, span) => {
                    let filter = get_or_lookup_local(&mut loaded_filters, *local_id, || {
                        state.env.get_filter(name)
                    })
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::UnknownFilter,
                            format!("filter {name} is unknown"),
                        )
                    })
                    .map_err(|e| state.with_span_error(e, span))?;
                    let args = stack.get_call_args(*arg_count);
                    let arg_count = args.len();
                    // An overridden argument (including the piped-in value)
                    // short-circuits the real filter: the filter never sees
                    // the original value, and the override is passed through.
                    let overridden = override_listener
                        .and_then(|l| args.iter().find_map(|v| l.override_value(v)));
                    let a = match overridden {
                        Some(v) => v,
                        None => filter
                            .apply_to(state, args)
                            .map_err(|e| state.with_span_error(e, span))?,
                    };
                    stack.drop_top(arg_count);
                    stack.push(a);
                }
                Instruction::PerformTest(name, arg_count, local_id, span) => {
                    let test = get_or_lookup_local(&mut loaded_tests, *local_id, || {
                        state.env.get_test(name)
                    })
                    .ok_or_else(|| {
                        Error::new(ErrorKind::UnknownTest, format!("test {name} is unknown"))
                    })
                    .map_err(|e| state.with_span_error(e, span))?;
                    let args = stack.get_call_args(*arg_count);
                    let arg_count = args.len();
                    // Same rule as filters: an overridden argument
                    // short-circuits the real test, propagating the override
                    // instead of a bool. `{% if %}` branching
                    // (`RenderingEventListener::override_branch`) checks with
                    // the listener before treating this as a real boolean, so
                    // this is safe even though the pushed value isn't
                    // actually a bool.
                    let overridden = override_listener
                        .and_then(|l| args.iter().find_map(|v| l.override_value(v)));
                    let rv = match overridden {
                        Some(v) => v,
                        None => Value::from(test.perform(state, args)?),
                    };
                    stack.drop_top(arg_count);
                    stack.push(rv);
                }
                Instruction::CallFunction(name, arg_count, _, this_span, _) => {
                    // reset_span is a special function that resets the current span
                    // it is only used internally like in run_operation.rs,
                    // where we want to set the location to yml file
                    if *name == "reset_span" {
                        let args = stack.get_call_args(*arg_count);
                        let arg_count = args.len();
                        let file_path = args[0].as_str().unwrap_or_default();
                        let start_line = args[1].as_i64().unwrap_or_default() as u32;
                        let start_col = args[2].as_i64().unwrap_or_default() as u32;
                        let start_offset = args[3].as_i64().unwrap_or_default() as u32;
                        let end_line = args[4].as_i64().unwrap_or_default() as u32;
                        let end_col = args[5].as_i64().unwrap_or_default() as u32;
                        let end_offset = args[6].as_i64().unwrap_or_default() as u32;
                        state.ctx.current_path = PathBuf::from(file_path);
                        state.ctx.current_span = Span {
                            start_line: start_line.saturating_sub(this_span.end_line),
                            start_col,
                            start_offset,
                            end_line: end_line.saturating_sub(this_span.end_line),
                            end_col,
                            end_offset,
                        };
                        stack.drop_top(arg_count);
                        stack.push(Value::UNDEFINED);
                        pc += 1;
                        continue;
                    }
                    // if return is called here, it means return is not on the top level of block
                    // e.g. {{ return(1) + 1 }}
                    // we should warn it
                    if *name == "return" {
                        listeners.iter().for_each(|listener| {
                            listener.on_malicious_return(&CodeLocation::new(
                                this_span.start_line,
                                this_span.start_col,
                                state.ctx.current_path.clone(),
                            ));
                        });
                        break;
                    }

                    let args = stack.get_call_args(*arg_count);
                    // super is a special function reserved for super-ing into blocks.
                    let rv = if *name == "super" {
                        if !args.is_empty() {
                            return Err(state.with_span_error(
                                Error::new(
                                    ErrorKind::InvalidOperation,
                                    "super() takes no arguments",
                                ),
                                this_span,
                            ));
                        }
                        self.perform_super(state, listeners)
                            .map_err(|e| state.with_span_error(e, this_span))?
                    // loop is a special name which when called recurses the current loop.
                    } else if *name == "loop" {
                        if args.len() != 1 {
                            return Err(state.with_span_error(
                                Error::new(
                                    ErrorKind::InvalidOperation,
                                    "loop() takes one argument",
                                ),
                                this_span,
                            ));
                        }
                        // leave the one argument on the stack for the recursion.  The
                        // recurse_loop! macro itself will perform a jump and not return here.
                        recurse_loop!(true, &this_span);
                    } else if (*name == "ref" || *name == "source" || *name == "function") && {
                        // we only consider the ref source override in root package
                        let template_result = self.env.get_template(name).or_else(|_| {
                            self.env
                                .get_template(&format!("{}.{}", root_package_name, *name))
                        });
                        template_result.is_ok()
                    } {
                        let resolved_name = if self.env.get_template(name).is_ok() {
                            (*name).to_string()
                        } else {
                            format!("{}.{}", root_package_name, *name)
                        };
                        listeners
                            .iter()
                            .for_each(|l| l.on_macro_dependency(&resolved_name));
                        let template = self
                            .env
                            .get_template(name)
                            .or_else(|_| {
                                self.env
                                    .get_template(&format!("{}.{}", root_package_name, *name))
                            })
                            .unwrap();
                        let template_registry_entry = template_registry.get(&Value::from(*name));
                        let path = template_registry_entry
                            .and_then(|entry| entry.get_attr_fast("path"))
                            .unwrap_or_else(|| Value::from(*name));
                        let span = template_registry_entry
                            .and_then(|entry| entry.get_attr_fast("span"))
                            .unwrap_or_else(|| Value::from_serialize(Span::default()));

                        let context = state.get_base_context_with_path_and_span(&path, &span);

                        let inner_state: State<'_, '_> = template
                            .eval_to_state_with_outer_stack_depth(
                                context,
                                listeners,
                                state.ctx.depth() + INCLUDE_RECURSION_COST,
                            )?;
                        let func = inner_state.lookup(name, listeners).unwrap();
                        call_wrapper(listeners, || func.call(&inner_state, args, listeners))
                            .map_err(|err| state.with_span_error(err, this_span))?
                    } else if let Some(func) = state
                        .lookup(name, listeners)
                        .filter(|func| !func.is_undefined())
                    {
                        let function_name = func
                            .get_attr_fast("function_name")
                            .map(|x| x.to_string())
                            .unwrap_or_else(|| (*name).to_string());

                        // Notify listeners about ref/source calls for mangled ref detection
                        if function_name == "ref" || function_name == "source" {
                            listeners.iter().for_each(|listener| {
                                listener.on_ref_or_source(
                                    &function_name,
                                    this_span.start_line,
                                    this_span.start_col,
                                    this_span.start_offset,
                                    this_span.end_line,
                                    this_span.end_col,
                                    this_span.end_offset,
                                );
                            });
                        }

                        // Notify listeners around the call with the resolved name and
                        // args so they can observe the rendered-output position before
                        // and after. Domain-agnostic: the vm attaches no meaning to the
                        // function (e.g. reclassify span capture lives in a listener).
                        listeners.iter().for_each(|listener| {
                            listener.on_function_call_start(&function_name, args);
                        });

                        state.record_pending_call_site(listeners, this_span);

                        // An overridden argument short-circuits a plain (non-macro)
                        // callable the same way `ApplyFilter`/`PerformTest`/`In` already
                        // short-circuit filters/tests/membership tests: a native function
                        // like `range()` expects a concretely-typed argument (e.g. `i32`)
                        // and has no taint-awareness of its own, so calling it with a
                        // fabricated stub value would otherwise raise a hard render error
                        // (e.g. "cannot convert plain object to i32") instead of degrading
                        // like every other operation on tainted values. Macro calls are
                        // exempted -- their own call-boundary taint rule (`Macro::call`)
                        // requires the macro body to actually execute so it can absorb
                        // taint from real internal logic, not just re-push an argument.
                        let overridden = if func.downcast_object::<Macro>().is_none() {
                            override_listener
                                .and_then(|l| args.iter().find_map(|v| l.override_value(v)))
                        } else {
                            None
                        };
                        let rv = match overridden {
                            Some(v) => v,
                            None => call_wrapper(listeners, || func.call(state, args, listeners))
                                .map_err(|err| state.with_span_error(err, this_span))?,
                        };

                        listeners.iter().for_each(|listener| {
                            listener.on_function_call_end(&function_name);
                        });

                        // Handle CallerReturn: when a return() was called inside a {% call %} block,
                        // the caller() function wraps it in CallerReturn. We unwrap it here and
                        // mark this frame as an explicit return so the value propagates out of
                        // the enclosing macro (2nd level of propagation). See CallerReturn docs.
                        if let Some(obj) = rv.as_object() {
                            if let Some(caller_return) = obj.downcast_ref::<CallerReturn>() {
                                stack.push(caller_return.value.clone());
                                is_explicit_return = true;
                                break;
                            }
                        }
                        rv
                    } else if *name == "render" {
                        let raw = args[0].as_str().unwrap_or_default();
                        let template = state.env().template_from_str(raw)?;
                        let rendered_sql = template.render(state.get_base_context(), listeners)?;
                        Value::from(rendered_sql)
                    } else {
                        return Err(state.with_span_error(
                            Error::new(
                                ErrorKind::UnknownFunction,
                                format!("Jinja macro or function `{name}` is unknown"),
                            ),
                            this_span,
                        ));
                    };
                    // Render the result of var function if it it has macro calls
                    let rv = if (name == &"var" || name == &"env_var")
                        && (rv.as_str().unwrap_or_default().contains("{{")
                            || rv.as_str().unwrap_or_default().contains("{#")
                            || rv.as_str().unwrap_or_default().contains("{%"))
                    {
                        let rv = self.env.render_str(
                            rv.as_str().unwrap(),
                            state.get_base_context(),
                            &[],
                        )?;
                        Value::from(rv)
                    } else {
                        rv
                    };
                    // A function/macro's own body may lose track of an
                    // overridden argument (e.g. absorbing it into a plain
                    // list via `namespace()`/`.append()` before emitting the
                    // joined result) -- same "any overridden input overrides
                    // the output" rule as binary operators/filters/tests,
                    // applied at the call boundary so it holds regardless of
                    // what the callee does internally.
                    let rv = override_listener
                        .and_then(|l| {
                            l.override_value(&rv)
                                .or_else(|| args.iter().find_map(|v| l.override_value(v)))
                        })
                        .unwrap_or(rv);
                    let arg_count = args.len();
                    stack.drop_top(arg_count);
                    stack.push(rv);
                }
                Instruction::CallMethod(name, arg_count, _, this_span, receiver_name) => {
                    let args = stack.get_call_args(*arg_count);
                    let arg_count = args.len();

                    let a = {
                        let args_vals: Vec<Value> = args[1..].to_vec();
                        let res = call_wrapper(listeners, || {
                            args[0].call_method(state, name, &args_vals, listeners)
                        });
                        match res {
                            Ok(rv) => match rv.downcast_object::<DispatchObject>() {
                                // If we return DispatchObject from a
                                // method call, we immediately forward
                                // the call to the dispatch object.
                                Some(obj) if obj.auto_execute => {
                                    state.record_pending_call_site(listeners, this_span);
                                    call_wrapper(listeners, || {
                                        obj.call(state, &args[1..], listeners)
                                    })
                                    .map_err(|e| state.with_span_error(e, this_span))?
                                }
                                _ => rv,
                            },
                            Err(err) => {
                                // try arg[0].get_property(name)(args[1..])
                                if let Some(property) = args[0]
                                    .as_object()
                                    .and_then(|obj| obj.get_property(state, name, listeners).ok())
                                {
                                    state.record_pending_call_site(listeners, this_span);
                                    call_wrapper(listeners, || {
                                        property.call(state, &args[1..], listeners)
                                    })
                                    .map_err(|e| state.with_span_error(e, this_span))?
                                } else {
                                    return Err(state.with_span_error(
                                        name_undefined_receiver(
                                            err,
                                            &args[0],
                                            *receiver_name,
                                            name,
                                        ),
                                        this_span,
                                    ));
                                }
                            }
                        }
                    };
                    // See the matching comment in `Instruction::CallFunction`:
                    // an overridden argument overrides the result even if the
                    // callee's own body loses track of it internally.
                    let a = override_listener
                        .and_then(|l| {
                            l.override_value(&a)
                                .or_else(|| args[1..].iter().find_map(|v| l.override_value(v)))
                        })
                        .unwrap_or(a);
                    stack.drop_top(arg_count);
                    stack.push(a);
                }
                Instruction::CallObject(arg_count, span) => {
                    let args = stack.get_call_args(*arg_count);
                    let arg_count = args.len();
                    state.record_pending_call_site(listeners, span);
                    let a = call_wrapper(listeners, || args[0].call(state, &args[1..], listeners))
                        .map_err(|e| state.with_span_error(e, span))?;
                    // See the matching comment in `Instruction::CallFunction`.
                    let a = override_listener
                        .and_then(|l| {
                            l.override_value(&a)
                                .or_else(|| args[1..].iter().find_map(|v| l.override_value(v)))
                        })
                        .unwrap_or(a);
                    stack.drop_top(arg_count);
                    stack.push(a);
                }
                Instruction::DupTop => {
                    stack.push(stack.peek().clone());
                }
                Instruction::DiscardTop => {
                    stack.pop();
                }
                Instruction::FastSuper(span) => {
                    out.write_str(
                        self.perform_super(state, listeners)
                            .map_err(|e| state.with_span_error(e, span))?
                            .as_str()
                            .unwrap_or_default(),
                    )?;
                }
                Instruction::FastRecurse(span) => {
                    recurse_loop!(false, &span);
                }
                // Explanation on the behavior of `LoadBlocks` and rendering of
                // inherited templates:
                //
                // MiniJinja inherits the behavior from Jinja2 where extending
                // loads the blocks (`LoadBlocks`) and the rest of the template
                // keeps executing but with output disabled, only at the end the
                // parent template is then invoked.  This has the effect that
                // you can still set variables or declare macros and that they
                // become visible in the blocks.
                //
                // This behavior has a few downsides.  First of all what happens
                // in the parent template overrides what happens in the child.
                // For instance if you declare a macro named `foo` after `{%
                // extends %}` and then a variable with that named is also set
                // in the parent template, then you won't be able to call that
                // macro in the body.
                //
                // The reason for this is that blocks unlike macros do not have
                // closures in Jinja2/MiniJinja.
                //
                // However for the common case this is convenient because it
                // lets you put some imports there and for as long as you do not
                // create name clashes this works fine.
                #[cfg(feature = "multi_template")]
                Instruction::LoadBlocks(span) => {
                    let a = stack.pop();
                    if parent_instructions.is_some() {
                        return Err(state.with_span_error(
                            Error::new(
                                ErrorKind::InvalidOperation,
                                "tried to extend a second time in a template",
                            ),
                            span,
                        ));
                    }
                    parent_instructions = Some(
                        self.load_blocks(a, state)
                            .map_err(|e| state.with_span_error(e, span))?,
                    );
                    out.begin_capture(CaptureMode::Discard);
                }
                #[cfg(feature = "multi_template")]
                Instruction::Include(ignore_missing, span) => {
                    let a = stack.pop();
                    out.write_str(
                        self.perform_include(a, state, *ignore_missing, listeners)
                            .map_err(|e| state.with_span_error(e, span))?
                            .as_str(),
                    )?;
                }
                #[cfg(feature = "multi_template")]
                Instruction::ExportLocals => {
                    let locals = state.ctx.current_locals_mut();
                    let mut module = value_map_with_capacity(locals.len());
                    for (key, value) in locals.iter() {
                        module.insert(Value::from(*key), value.clone());
                    }
                    stack.push(Value::from_object(module));
                }
                #[cfg(feature = "macros")]
                Instruction::BuildMacro(name, offset, flags, _) => {
                    self.build_macro(&mut stack, state, *offset, name, *flags);
                }
                #[cfg(feature = "macros")]
                Instruction::Return {
                    explicit,
                    arg_count,
                } => {
                    if let Some(arg_count) = arg_count {
                        let args = stack.get_call_args(*arg_count);
                        let arg_count = args.len();
                        if arg_count != 1 {
                            return Err(Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                "Incorrect return argument count",
                            ));
                        }
                    }
                    is_explicit_return = *explicit;
                    if *explicit && current_macro_name == Some("caller".to_string()) {
                        is_caller_return = true;
                    }
                    break;
                }
                #[cfg(feature = "macros")]
                Instruction::Enclose(name) => {
                    // the first time we enclose a value, we need to create a closure
                    // and store it on the context, and add it to the closure tracker
                    // for cycle breaking.
                    if state.ctx.closure().is_none() {
                        let closure = Arc::new(Closure::default());
                        state.closure_tracker.track_closure(closure.clone());
                        state.ctx.reset_closure(Some(closure));
                    }

                    let value = state.lookup(name, listeners).unwrap_or(Value::UNDEFINED);
                    state.ctx.enclose(name, value);
                }
                #[cfg(feature = "macros")]
                Instruction::GetClosure => {
                    stack.push(
                        state
                            .ctx
                            .closure()
                            .map_or(Value::UNDEFINED, |x| Value::from_dyn_object(x.clone())),
                    );
                }
                Instruction::MacroStart(line, col, index) => {
                    if current_macro_name.is_none() {
                        let path = state.ctx.current_path.as_path();
                        let span = state.ctx.current_span;
                        let line = span.start_line + *line - 1;
                        let col = *col
                            + if span.start_line == 1 {
                                span.start_col - 1
                            } else {
                                0
                            };
                        let offset = *index + span.start_offset;
                        listeners.iter().for_each(|listener| {
                            listener.on_macro_start(Some(path), &line, &col, &offset)
                        });
                    }
                }
                Instruction::MacroStop(line, col, index) => {
                    if current_macro_name.is_none() {
                        let path = state.ctx.current_path.as_path();
                        let span = state.ctx.current_span;
                        let line = span.start_line + *line - 1;
                        let col = *col
                            + if span.start_line == 1 {
                                span.start_col - 1
                            } else {
                                0
                            };
                        let offset = *index + span.start_offset;
                        listeners.iter().for_each(|listener| {
                            listener.on_macro_stop(Some(path), &line, &col, &offset)
                        });
                    }
                }
                Instruction::JinjaLayoutEvent(kind, source_span) => {
                    if current_macro_name.is_none() {
                        let source_span =
                            span_with_current_offset(source_span, &state.ctx.current_span);
                        listeners.iter().for_each(|listener| {
                            listener.on_jinja_layout_event(*kind, &source_span)
                        });
                    }
                }
                Instruction::JinjaLayoutEventIfLoopDidNotIterate(_kind, source_span) => {
                    let did_not_iterate = state
                        .ctx
                        .current_loop()
                        .is_some_and(|loop_ctx| loop_ctx.object.idx.load(Ordering::Relaxed) == 0);
                    if did_not_iterate && current_macro_name.is_none() {
                        let source_span =
                            span_with_current_offset(source_span, &state.ctx.current_span);
                        listeners
                            .iter()
                            .for_each(|listener| listener.on_jinja_loop_skipped_end(&source_span));
                    }
                }
                Instruction::JinjaLayoutLoopIterationStart(source_span) => {
                    let is_repeated_iteration = state
                        .ctx
                        .current_loop()
                        .is_some_and(|loop_ctx| loop_ctx.object.idx.load(Ordering::Relaxed) > 0);
                    if is_repeated_iteration && current_macro_name.is_none() {
                        let source_span =
                            span_with_current_offset(source_span, &state.ctx.current_span);
                        listeners.iter().for_each(|listener| {
                            listener.on_jinja_loop_iteration_start(&source_span)
                        });
                    }
                }
                Instruction::MacroName(name, _) => {
                    current_macro_name = Some((*name).to_string());
                }
                Instruction::TypeConstraint(_type_constraint, _is_true, _span) => {
                    // no-op, we don't need to do anything here
                }
                Instruction::LoadType(_type) => {
                    // no-op, we don't need to do anything here
                }
                Instruction::UnionType => {
                    // no-op, we don't need to do anything here
                }
            }
            pc += 1;
        }

        // dbg!(&rv);
        if is_caller_return {
            let rv = stack.pop();
            Ok(Value::from_object(CallerReturn::new(rv)))
        } else if is_explicit_return {
            Ok(stack.pop())
        } else {
            Ok(Value::from(rv))
        }
    }

    #[cfg(feature = "multi_template")]
    #[allow(clippy::too_many_arguments)]
    fn perform_include(
        &self,
        name: Value,
        state: &mut State<'_, 'env>,
        ignore_missing: bool,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<String, Error> {
        let obj = name.as_object();
        let choices = obj
            .as_ref()
            .and_then(|d| d.try_iter())
            .into_iter()
            .flatten()
            .chain(obj.is_none().then(|| name.clone()));

        let mut templates_tried = vec![];
        for choice in choices {
            let name = ok!(choice.as_str().ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    "template name was not a string",
                )
            }));
            let tmpl = match state.get_template(name) {
                Ok(tmpl) => tmpl,
                Err(err) => {
                    if err.kind() == ErrorKind::TemplateNotFound {
                        templates_tried.push(choice);
                    } else {
                        return Err(err);
                    }
                    continue;
                }
            };

            let (new_instructions, new_blocks) = ok!(tmpl.instructions_and_blocks());
            let old_escape = mem::replace(&mut state.auto_escape, tmpl.initial_auto_escape());
            let old_instructions = mem::replace(&mut state.instructions, new_instructions);
            let old_blocks = mem::replace(&mut state.blocks, prepare_blocks(new_blocks));
            // we need to make a copy of the loaded templates here as we want
            // to forget about the templates that an include triggered by the
            // time the include finishes.
            let old_loaded_templates = state.loaded_templates.clone();
            ok!(state.ctx.incr_depth(INCLUDE_RECURSION_COST));
            let rv;
            #[cfg(feature = "macros")]
            {
                let old_closure = state.ctx.take_closure();
                rv = self.eval_state(state, listeners);
                state.ctx.reset_closure(old_closure);
            }
            #[cfg(not(feature = "macros"))]
            {
                rv = self.eval_state(state, out);
            }
            state.ctx.decr_depth(INCLUDE_RECURSION_COST);
            state.loaded_templates = old_loaded_templates;
            state.auto_escape = old_escape;
            state.instructions = old_instructions;
            state.blocks = old_blocks;
            let rv = ok!(rv.map_err(|err| {
                Error::new(
                    ErrorKind::BadInclude,
                    format!("error in \"{}\"", tmpl.name()),
                )
                .with_source(err)
            }));
            return Ok(rv.as_str().unwrap().to_string());
        }
        if !templates_tried.is_empty() && !ignore_missing {
            Err(Error::new(
                ErrorKind::TemplateNotFound,
                if templates_tried.len() == 1 {
                    format!(
                        "tried to include non-existing template {:?}",
                        templates_tried[0]
                    )
                } else {
                    format!(
                        "tried to include one of multiple templates, none of which existed {}",
                        Value::from(templates_tried)
                    )
                },
            ))
        } else {
            Ok(String::new())
        }
    }

    fn perform_super(
        &self,
        state: &mut State<'_, 'env>,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let name = ok!(state.current_block.ok_or_else(|| {
            Error::new(ErrorKind::InvalidOperation, "cannot super outside of block")
        }));

        let block_stack = state.blocks.get_mut(name).unwrap();
        if !block_stack.push() {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "no parent block exists",
            ));
        }

        let old_instructions = mem::replace(&mut state.instructions, block_stack.instructions());
        ok!(state.ctx.push_frame(Frame::default()));
        let rv = self.eval_state(state, listeners);
        state.ctx.pop_frame();
        state.instructions = old_instructions;
        state.blocks.get_mut(name).unwrap().pop();

        rv.map_err(|err| Error::new(ErrorKind::EvalBlock, "error in super block").with_source(err))
    }

    fn prepare_loop_recursion(&self, state: &mut State) -> Result<usize, Error> {
        if let Some(loop_ctx) = state.ctx.current_loop() {
            if let Some(recurse_jump_target) = loop_ctx.recurse_jump_target {
                Ok(recurse_jump_target)
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "cannot recurse outside of recursive loop",
                ))
            }
        } else {
            Err(Error::new(
                ErrorKind::InvalidOperation,
                "cannot recurse outside of loop",
            ))
        }
    }

    #[cfg(feature = "multi_template")]
    fn load_blocks(
        &self,
        name: Value,
        state: &mut State<'_, 'env>,
    ) -> Result<&'env Instructions<'env>, Error> {
        let name = match name.as_str() {
            Some(name) => name,
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "template name was not a string",
                ))
            }
        };
        if state.loaded_templates.contains(&name) {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("cycle in template inheritance. {name:?} was referenced more than once"),
            ));
        }
        let tmpl = ok!(state.get_template(name));
        let (new_instructions, new_blocks) = ok!(tmpl.instructions_and_blocks());
        state.loaded_templates.insert(new_instructions.name());
        for (name, instr) in new_blocks.iter() {
            state
                .blocks
                .entry(name)
                .or_default()
                .append_instructions(instr);
        }
        Ok(new_instructions)
    }

    #[cfg(feature = "multi_template")]
    pub(crate) fn call_block(
        &self,
        name: &str,
        state: &mut State<'_, 'env>,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        if let Some((name, block_stack)) = state.blocks.get_key_value(name) {
            let old_block = state.current_block.replace(name);
            let old_instructions =
                mem::replace(&mut state.instructions, block_stack.instructions());
            state.ctx.push_frame(Frame::default())?;
            let rv = self.eval_state(state, listeners);
            state.ctx.pop_frame();
            state.instructions = old_instructions;
            state.current_block = old_block;
            rv
        } else {
            Err(Error::new(
                ErrorKind::UnknownBlock,
                format!("block '{name}' not found"),
            ))
        }
    }
    fn derive_auto_escape(
        &self,
        value: Value,
        initial_auto_escape: AutoEscape,
    ) -> Result<AutoEscape, Error> {
        match (value.as_str(), value == Value::from(true)) {
            (Some("html"), _) => Ok(AutoEscape::Html),
            #[cfg(feature = "json")]
            (Some("json"), _) => Ok(AutoEscape::Json),
            (Some("none"), _) | (None, false) => Ok(AutoEscape::None),
            (None, true) => Ok(if matches!(initial_auto_escape, AutoEscape::None) {
                AutoEscape::Html
            } else {
                initial_auto_escape
            }),
            _ => Err(Error::new(
                ErrorKind::InvalidOperation,
                "invalid value to autoescape tag",
            )),
        }
    }

    fn push_loop(
        &self,
        state: &mut State<'_, 'env>,
        iterable: Value,
        flags: u8,
        pc: usize,
        current_recursion_jump: Option<(usize, bool)>,
    ) -> Result<(), Error> {
        let mut iterator = ok!(state.undefined_behavior().try_iter(iterable));
        // for an iterator where the lower and upper bound are matching we can
        // consider them to have ExactSizeIterator semantics.  We do however not
        // expect ExactSizeIterator bounds themselves to support iteration by
        // other means.
        let len = match iterator.size_hint() {
            (lower, Some(upper)) if lower == upper => Some(lower),
            _ => None,
        };
        let depth = state
            .ctx
            .current_loop()
            .filter(|x| x.recurse_jump_target.is_some())
            .map_or(0, |x| x.object.depth + 1);
        let recursive = flags & LOOP_FLAG_RECURSIVE != 0;
        let with_loop_var = flags & LOOP_FLAG_WITH_LOOP_VAR != 0;
        ok!(state.ctx.push_frame(Frame {
            current_loop: Some(LoopState {
                with_loop_var,
                recurse_jump_target: if recursive { Some(pc) } else { None },
                current_recursion_jump,
                object: Arc::new(Loop {
                    idx: AtomicUsize::new(!0usize),
                    len,
                    depth,
                    #[cfg(feature = "adjacent_loop_items")]
                    value_triple: Mutex::new((None, None, iterator.next())),
                    last_changed_value: Mutex::default(),
                }),
                iterator,
            }),
            ..Frame::default()
        }));
        Ok(())
    }

    fn unpack_list(&self, stack: &mut Stack, count: usize) -> Result<(), Error> {
        let top = stack.pop();
        if let Some(stub) = top.downcast_object::<IntrospectiveValue>() {
            // Fixed-arity destructuring (`{% set a, b = call() %}`, and each
            // item of a `{% for a, b in list %}`) also compiles to
            // `UnpackList` and shares this VM path with for-loops, but
            // `IntrospectiveValue::enumerate` always yields exactly one
            // representative item regardless of arity (see its doc comment
            // -- that invariant is specifically for loop bodies). Delegate
            // to `IntrospectiveValue::unpack`, which hands back exactly
            // `count` tainted items (preserving the fabricated inner
            // value's real items when their count already matches) instead
            // of forcing the value through that single-item iterator, which
            // would otherwise fail every tainted tuple-unpack with
            // "sequence of wrong length" (e.g. `{% set res, table =
            // adapter.execute(...) %}` in dbt-adapters' `statement()`
            // macro). Checking for the concrete wrapper type here (rather
            // than the broader `is_introspective_stub()`) is deliberate: an
            // ordinary container that merely *contains* a tainted item (see
            // e.g. `appending_a_tainted_item_taints_the_list`) doesn't have
            // this single-item invariant and unpacks fine below.
            for item in stub.unpack(count) {
                stack.push(item);
            }
            return Ok(());
        }
        let iter = ok!(top
            .as_object()
            .and_then(|x| x.try_iter())
            .ok_or_else(|| Error::new(ErrorKind::CannotUnpack, "value is not iterable")));

        let mut n = 0;
        for item in iter {
            stack.push(item);
            n += 1;
        }

        if n == count {
            stack.reverse_top(n);
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::CannotUnpack,
                format!("sequence of wrong length (expected {count}, got {n})",),
            ))
        }
    }

    #[cfg(feature = "macros")]
    fn build_macro(
        &self,
        stack: &mut Stack,
        state: &mut State,
        offset: usize,
        name: &str,
        flags: u8,
    ) {
        use crate::{compiler::instructions::MACRO_CALLER, vm::macro_object::Macro};
        let arg_spec = stack.pop().try_iter().unwrap().collect();
        let closure = stack.pop();
        let macro_ref_id = state.macros.len();
        Arc::make_mut(&mut state.macros).push((state.instructions, offset));
        // dbt registers each macro as a template named `{package}.{macro_name}`,
        // so the enclosing template's name yields the package this macro belongs to.
        let package_name = state
            .instructions
            .name()
            .split_once('.')
            .map(|(package, _)| package.to_string());
        let m = Macro {
            name: Value::from(name),
            package_name,
            arg_spec,
            macro_ref_id,
            state_id: state.id,
            closure,
            caller_reference: (flags & MACRO_CALLER) != 0,
            path: state.ctx.current_path.clone(),
            span: state.ctx.current_span,
        };
        stack.push(Value::from_object(m));
    }
}

/// Deserialize a path from a value
pub fn deserialize_path(value: &Value) -> PathBuf {
    PathBuf::from(value.as_str().unwrap())
}

/// Deserialize a span from a value
pub fn deserialize_span(value: &Value) -> Span {
    Span {
        start_line: value
            .get_attr_fast("start_line")
            .unwrap()
            .as_usize()
            .unwrap() as u32,
        start_col: value
            .get_attr_fast("start_col")
            .unwrap()
            .as_usize()
            .unwrap() as u32,
        start_offset: value
            .get_attr_fast("start_offset")
            .unwrap()
            .as_usize()
            .unwrap() as u32,
        end_line: value.get_attr_fast("end_line").unwrap().as_usize().unwrap() as u32,
        end_col: value.get_attr_fast("end_col").unwrap().as_usize().unwrap() as u32,
        end_offset: value
            .get_attr_fast("end_offset")
            .unwrap()
            .as_usize()
            .unwrap() as u32,
    }
}
