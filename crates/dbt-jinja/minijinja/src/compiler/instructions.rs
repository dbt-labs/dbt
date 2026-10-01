#[cfg(feature = "internal_debug")]
use std::fmt;

use crate::compiler::codegen::TypeConstraint;
use crate::compiler::tokens::Span;
use crate::layout::JinjaLayoutEventKind;
use crate::output::CaptureMode;
use crate::value::Value;

/// This loop has the loop var.
pub const LOOP_FLAG_WITH_LOOP_VAR: u8 = 1;

/// This loop is recursive.
pub const LOOP_FLAG_RECURSIVE: u8 = 2;

/// This macro uses the caller var.
#[cfg(feature = "macros")]
pub const MACRO_CALLER: u8 = 2;

/// Rust type to represent locals.
pub type LocalId = u8;

/// The maximum number of filters/tests that can be cached.
pub const MAX_LOCALS: usize = 50;

/// Represents an instruction for the VM.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(
    feature = "unstable_machinery_serde",
    derive(serde::Serialize),
    serde(tag = "op", content = "arg")
)]
#[derive(Clone)]
pub enum Instruction<'source> {
    /// Emits raw source
    EmitRaw(&'source str, Span),

    /// Stores a variable (only possible in for loops)
    StoreLocal(&'source str, Span),

    /// Load a variable,
    Lookup(&'source str, Span),

    /// Looks up an attribute.
    GetAttr(&'source str, Span),

    /// Sets an attribute.
    SetAttr(&'source str, Span),

    /// Looks up an item.
    GetItem(Span),

    /// Performs a slice operation.
    Slice(Span),

    /// Loads a constant value.
    LoadConst(Value),

    /// Builds a map of the last n pairs on the stack.
    BuildMap(usize, Span),

    /// Builds a kwargs map of the last n pairs on the stack.
    BuildKwargs(usize),

    /// Merges N kwargs maps on the list into one.
    MergeKwargs(usize),

    /// Builds a list of the last n pairs on the stack.
    BuildList(Option<usize>, Span),

    /// Builds a tuple of the last n pairs on the stack.
    BuildTuple(Option<usize>, Span),

    /// Unpacks a list into N stack items.
    UnpackList(usize, Span),

    /// Unpacks N lists onto the stack and pushes the number of items there were unpacked.
    UnpackLists(usize, Span),

    /// Add the top two values
    Add(Span),

    /// Subtract the top two values
    Sub(Span),

    /// Multiply the top two values
    Mul(Span),

    /// Divide the top two values
    Div(Span),

    /// Integer divide the top two values as "integer".
    ///
    /// Note that in MiniJinja this currently uses an euclidean
    /// division to match the rem implementation.  In Python this
    /// instead uses a flooring division and a flooring remainder.
    IntDiv(Span),

    /// Calculate the remainder the top two values
    Rem(Span),

    /// x to the power of y.
    Pow(Span),

    /// Negates the value.
    Neg(Span),

    /// Unary positive.
    Pos(Span),

    /// `=` operator
    Eq(Span),

    /// `!=` operator
    Ne(Span),

    /// `>` operator
    Gt(Span),

    /// `>=` operator
    Gte(Span),

    /// `<` operator
    Lt(Span),

    /// `<=` operator
    Lte(Span),

    /// Unary not
    Not(Span),

    /// String concatenation operator
    StringConcat(Span),

    /// Performs a containment check
    In(Span),

    /// Apply a filter.
    ApplyFilter(&'source str, Option<u16>, LocalId, Span),

    /// Perform a filter.
    PerformTest(&'source str, Option<u16>, LocalId, Span),

    /// Emit the stack top as output
    Emit(Span),

    /// Starts a loop
    ///
    /// The argument are loop flags.
    PushLoop(u8, Span),

    /// Starts a with block.
    PushWith(Span),

    /// Does a single loop iteration
    ///
    /// The argument is the jump target for when the loop
    /// ends and must point to a `PopFrame` instruction.
    Iterate(usize, Span),

    /// Push a bool that indicates that the loop iterated.
    PushDidNotIterate,

    /// Pops the topmost frame
    PopFrame,

    /// Jump to a specific instruction
    Jump(usize, Span),

    /// Jump if the stack top evaluates to false
    JumpIfFalse(usize, Span),

    /// Jump if the stack top evaluates to true
    JumpIfTrue(usize, Span),

    /// Jump if the stack top evaluates to false or pops the value
    JumpIfFalseOrPop(usize, Span),

    /// Jump if the stack top evaluates to true or pops the value
    JumpIfTrueOrPop(usize, Span),

    /// Sets the auto escape flag to the current value.
    PushAutoEscape(Span),

    /// Resets the auto escape flag to the previous value.
    PopAutoEscape,

    /// Begins capturing of output (false) or discard (true).
    BeginCapture(CaptureMode),

    /// Ends capturing of output.
    EndCapture,

    /// Calls a global function
    CallFunction(
        &'source str,
        Option<u16>,
        Box<Span>,
        Box<Span>,
        Option<Box<Span>>,
    ),

    /// Calls a method
    ///
    /// The last field carries the receiver's identifier when the receiver is a
    /// plain variable (`foo.bar()` -> `Some("foo")`), so that a failed call on an
    /// undefined receiver can name it. `None` for any other receiver expression.
    CallMethod(
        &'source str,
        Option<u16>,
        Box<Span>,
        Box<Span>,
        Option<&'source str>,
    ),

    /// Calls an object
    CallObject(Option<u16>, Span),

    /// Duplicates the top item
    DupTop,

    /// Discards the top item
    DiscardTop,

    /// A fast super instruction without intermediate capturing.
    FastSuper(Span),

    /// A fast loop recurse instruction without intermediate capturing.
    FastRecurse(Span),

    /// Swaps the top two items in the stack.
    Swap,

    /// Call into a block.
    #[cfg(feature = "multi_template")]
    CallBlock(&'source str),

    /// Loads block from a template with name on stack ("extends")
    #[cfg(feature = "multi_template")]
    LoadBlocks(Span),

    /// Includes another template.
    #[cfg(feature = "multi_template")]
    Include(bool, Span),

    /// Builds a module
    #[cfg(feature = "multi_template")]
    ExportLocals,

    /// Builds a macro on the stack.
    #[cfg(feature = "macros")]
    BuildMacro(&'source str, usize, u8, Span),

    /// Breaks from the interpreter loop (exists a function)
    #[cfg(feature = "macros")]
    Return {
        explicit: bool,
        arg_count: Option<Option<u16>>,
    },

    /// True if the value is undefined
    #[cfg(feature = "macros")]
    IsUndefined,

    /// Encloses a variable.
    #[cfg(feature = "macros")]
    Enclose(&'source str),

    /// Returns the closure of this context level.
    #[cfg(feature = "macros")]
    GetClosure,

    MacroStart(u32, u32, u32),
    MacroStop(u32, u32, u32),
    JinjaLayoutEvent(JinjaLayoutEventKind, Span),
    JinjaLayoutEventIfLoopDidNotIterate(JinjaLayoutEventKind, Span),
    JinjaLayoutLoopIterationStart(Span),

    // A label instruction to indicate the start of a macro
    // After MacroName, there will be a series of StoreLocal instructions for parameters
    // We need to push parameter types to the stack.
    MacroName(&'source str, Span),

    /// Type constraint is used to tell type checker to mutate the type of the variable
    TypeConstraint(TypeConstraint, bool, Span),

    /// Load a type to the stack
    LoadType(Value),

    /// Union the top two types
    UnionType,
}

#[derive(Copy, Clone)]
struct LineInfo {
    first_instruction: u32,
    line: u32,
}

#[derive(Clone)]
struct SpanInfo {
    first_instruction: u32,
    span: Option<Span>,
}

/// Wrapper around instructions to help with location management.
pub struct Instructions<'source> {
    pub(crate) instructions: Vec<Instruction<'source>>,
    line_infos: Vec<LineInfo>,
    span_infos: Vec<SpanInfo>,
    name: &'source str,
    source: &'source str,
    filename: Option<String>,
}

pub(crate) static EMPTY_INSTRUCTIONS: Instructions<'static> = Instructions {
    instructions: Vec::new(),
    line_infos: Vec::new(),
    span_infos: Vec::new(),
    name: "<unknown>",
    source: "",
    filename: None,
};

impl<'source> Instructions<'source> {
    /// Creates a new instructions object.
    pub fn new(
        name: &'source str,
        source: &'source str,
        filename: Option<String>,
    ) -> Instructions<'source> {
        Instructions {
            instructions: Vec::with_capacity(128),
            line_infos: Vec::with_capacity(128),
            span_infos: Vec::with_capacity(128),
            name,
            source,
            filename,
        }
    }

    /// Returns the name of the template.
    pub fn name(&self) -> &'source str {
        self.name
    }

    /// Returns the filename of the template.
    pub fn filename(&self) -> String {
        self.filename
            .clone()
            .unwrap_or_else(|| self.name.to_string())
    }

    /// Returns the source reference.
    pub fn source(&self) -> &'source str {
        self.source
    }

    /// Returns an instruction by index
    #[inline(always)]
    pub fn get(&self, idx: usize) -> Option<&Instruction<'source>> {
        self.instructions.get(idx)
    }

    /// Returns an instruction by index mutably
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut Instruction<'source>> {
        self.instructions.get_mut(idx)
    }

    /// Adds a new instruction
    pub fn add(&mut self, instr: Instruction<'source>) -> usize {
        let rv = self.instructions.len();
        self.instructions.push(instr);
        rv
    }

    fn add_line_record(&mut self, instr: usize, line: u32) {
        let same_loc = self
            .line_infos
            .last()
            .is_some_and(|last_loc| last_loc.line == line);
        if !same_loc {
            self.line_infos.push(LineInfo {
                first_instruction: instr as u32,
                line,
            });
        }
    }

    /// Adds a new instruction with line number.
    pub fn add_with_line(&mut self, instr: Instruction<'source>, line: u32) -> usize {
        let rv = self.add(instr);
        self.add_line_record(rv, line);

        // if we follow up to a valid span with no more span, clear it out
        {
            if self.span_infos.last().is_some_and(|x| x.span.is_some()) {
                self.span_infos.push(SpanInfo {
                    first_instruction: rv as u32,
                    span: None,
                });
            }
        }
        rv
    }

    /// Adds a new instruction with span.
    pub fn add_with_span(&mut self, instr: Instruction<'source>, span: Span) -> usize {
        let rv = self.add(instr);
        {
            let same_loc = self
                .span_infos
                .last()
                .is_some_and(|last_loc| last_loc.span == Some(span));
            if !same_loc {
                self.span_infos.push(SpanInfo {
                    first_instruction: rv as u32,
                    span: Some(span),
                });
            }
        }
        self.add_line_record(rv, span.start_line);
        rv
    }

    /// Looks up the line for an instruction
    pub fn get_line(&self, idx: usize) -> Option<usize> {
        let loc = match self
            .line_infos
            .binary_search_by_key(&idx, |x| x.first_instruction as usize)
        {
            Ok(idx) => &self.line_infos[idx],
            Err(0) => return None,
            Err(idx) => &self.line_infos[idx - 1],
        };
        Some(loc.line as usize)
    }

    /// Looks up a span for an instruction.
    pub fn get_span(&self, idx: usize) -> Option<Span> {
        let loc = match self
            .span_infos
            .binary_search_by_key(&idx, |x| x.first_instruction as usize)
        {
            Ok(idx) => &self.span_infos[idx],
            Err(0) => return None,
            Err(idx) => &self.span_infos[idx - 1],
        };
        loc.span
    }

    /// Returns a list of all names referenced in the current block backwards
    /// from the given pc.
    #[cfg(feature = "debug")]
    pub fn get_referenced_names(&self, idx: usize) -> Vec<&'source str> {
        let mut rv = Vec::new();
        // make sure we don't crash on empty instructions
        if self.instructions.is_empty() {
            return rv;
        }
        let idx = idx.min(self.instructions.len() - 1);
        for instr in self.instructions[..=idx].iter().rev() {
            let name = match instr {
                Instruction::Lookup(name, _)
                | Instruction::StoreLocal(name, _)
                | Instruction::CallFunction(name, _, _, _, _) => *name,
                Instruction::PushLoop(flags, _) if flags & LOOP_FLAG_WITH_LOOP_VAR != 0 => "loop",
                Instruction::PushLoop(_, _) | Instruction::PushWith(_) => break,
                _ => continue,
            };
            if !rv.contains(&name) {
                rv.push(name);
            }
        }
        rv
    }

    /// Returns the number of instructions
    pub fn len(&self) -> usize {
        self.instructions.len()
    }

    /// Do we have any instructions?
    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.instructions.is_empty()
    }
}

#[cfg(feature = "internal_debug")]
impl fmt::Debug for Instructions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct InstructionWrapper<'a>(usize, &'a Instruction<'a>, Option<usize>);

        impl fmt::Debug for InstructionWrapper<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                ok!(write!(f, "{:>05} | {:?}", self.0, self.1,));
                if let Some(line) = self.2 {
                    ok!(write!(f, "  [line {line}]"));
                }
                Ok(())
            }
        }

        let mut list = f.debug_list();
        let mut last_line = None;
        for (idx, instr) in self.instructions.iter().enumerate() {
            let line = self.get_line(idx);
            list.entry(&InstructionWrapper(
                idx,
                instr,
                if line != last_line { line } else { None },
            ));
            last_line = line;
        }
        list.finish()
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn test_sizes() {
    assert_eq!(std::mem::size_of::<Instruction>(), 96);
}
