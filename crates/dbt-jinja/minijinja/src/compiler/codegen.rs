use std::collections::BTreeMap;
use std::sync::Arc;

use crate::compiler::ast::{self, Expr};
use crate::compiler::instructions::{
    Instruction, Instructions, LocalId, LOOP_FLAG_RECURSIVE, LOOP_FLAG_WITH_LOOP_VAR, MAX_LOCALS,
};
use crate::compiler::tokens::Span;
use crate::compiler::typecheck::FunctionRegistry;
use crate::layout::JinjaLayoutEventKind;
use crate::output::CaptureMode;
use crate::types::function::UserDefinedFunctionType;
use crate::types::Type;
use crate::value::ops::neg;
use crate::value::{Kwargs, Value, ValueMap};
use crate::vm::typemeta::macro_namespace_template_resolver;
use crate::vm::typemeta::Part;

use serde::{Deserialize, Serialize};
#[cfg(test)]
use similar_asserts::assert_eq;

#[cfg(feature = "macros")]
type Caller<'source> = ast::Spanned<ast::Macro<'source>>;

#[cfg(not(feature = "macros"))]
type Caller<'source> = std::marker::PhantomData<&'source ()>;

/// For the first `MAX_LOCALS` filters/tests, an ID is returned for faster lookups from the stack.
fn get_local_id<'source>(ids: &mut BTreeMap<&'source str, LocalId>, name: &'source str) -> LocalId {
    if let Some(id) = ids.get(name) {
        *id
    } else if ids.len() >= MAX_LOCALS {
        !0
    } else {
        let next_id = ids.len() as LocalId;
        ids.insert(name, next_id);
        next_id
    }
}

fn get_identifier_span<'a>(expr: &ast::Expr<'a>) -> Option<Span> {
    match expr {
        Expr::Var(var) => Some(var.span()),
        Expr::GetAttr(get_attr) => Some(get_attr.span()),
        _ => None,
    }
}

/// Represents an open block of code that does not yet have updated
/// jump targets.
enum PendingBlock {
    Branch {
        jump_instr: usize,
    },
    Loop {
        iter_instr: usize,
        jump_instrs: Vec<usize>,
    },
    ScBool {
        jump_instrs: Vec<usize>,
    },
}

#[derive(Clone)]
pub enum CodeGenerationProfile {
    TypeCheck(Arc<FunctionRegistry>, BTreeMap<String, Value>),
    Render,
}

impl std::fmt::Debug for CodeGenerationProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeCheck(_arg0, _arg1) => write!(f, "Render"),
            Self::Render => write!(f, "Render"),
        }
    }
}

/// Provides a convenient interface to creating instructions for the VM.
pub struct CodeGenerator<'source> {
    instructions: Instructions<'source>,
    blocks: BTreeMap<&'source str, Instructions<'source>>,
    pending_block: Vec<PendingBlock>,
    current_line: u32,
    span_stack: Vec<Span>,
    filter_local_ids: BTreeMap<&'source str, LocalId>,
    test_local_ids: BTreeMap<&'source str, LocalId>,
    raw_template_bytes: usize,
    profile: CodeGenerationProfile,
}

impl<'source> CodeGenerator<'source> {
    /// Creates a new code generator.
    pub fn new(
        file: &'source str,
        source: &'source str,
        profile: CodeGenerationProfile,
    ) -> CodeGenerator<'source> {
        CodeGenerator {
            instructions: Instructions::new(file, source, None),
            blocks: BTreeMap::new(),
            pending_block: Vec::with_capacity(32),
            current_line: 0,
            span_stack: Vec::with_capacity(32),
            filter_local_ids: BTreeMap::new(),
            test_local_ids: BTreeMap::new(),
            raw_template_bytes: 0,
            profile,
        }
    }

    pub fn new_with_filename(
        file: &'source str,
        source: &'source str,
        filename: Option<String>,
        profile: CodeGenerationProfile,
    ) -> CodeGenerator<'source> {
        CodeGenerator {
            instructions: Instructions::new(file, source, filename),
            blocks: BTreeMap::new(),
            pending_block: Vec::with_capacity(32),
            current_line: 0,
            span_stack: Vec::with_capacity(32),
            filter_local_ids: BTreeMap::new(),
            test_local_ids: BTreeMap::new(),
            raw_template_bytes: 0,
            profile,
        }
    }

    /// Sets the current location's line.
    pub fn set_line(&mut self, lineno: u32) {
        self.current_line = lineno;
    }

    /// Sets line from span.
    pub fn set_line_from_span(&mut self, span: Span) {
        self.set_line(span.start_line);
    }

    /// Pushes a span to the stack
    pub fn push_span(&mut self, span: Span) {
        self.span_stack.push(span);
        self.set_line_from_span(span);
    }

    /// Pops a span from the stack.
    pub fn pop_span(&mut self) {
        self.span_stack.pop();
    }

    /// Add a simple instruction with the current location.
    pub fn add(&mut self, instr: Instruction<'source>) -> usize {
        if let Some(span) = self.span_stack.last() {
            if span.start_line == self.current_line {
                return self.instructions.add_with_span(instr, *span);
            }
        }
        self.instructions.add_with_line(instr, self.current_line)
    }

    /// Add a simple instruction with other location.
    pub fn add_with_span(&mut self, instr: Instruction<'source>, span: Span) -> usize {
        self.instructions.add_with_span(instr, span)
    }

    /// Returns the next instruction index.
    pub fn next_instruction(&self) -> usize {
        self.instructions.len()
    }

    /// Creates a sub generator.
    #[cfg(feature = "multi_template")]
    fn new_subgenerator(&self) -> CodeGenerator<'source> {
        let mut sub = CodeGenerator::new_with_filename(
            self.instructions.name(),
            self.instructions.source(),
            Some(self.instructions.filename()),
            self.profile.clone(),
        );
        sub.current_line = self.current_line;
        sub.span_stack = self.span_stack.last().cloned().into_iter().collect();
        sub
    }

    /// Finishes a sub generator and syncs it back.
    #[cfg(feature = "multi_template")]
    fn finish_subgenerator(&mut self, sub: CodeGenerator<'source>) -> Instructions<'source> {
        self.current_line = sub.current_line;
        let (instructions, blocks) = sub.finish();
        self.blocks.extend(blocks);
        instructions
    }

    /// Starts a for loop
    pub fn start_for_loop(&mut self, with_loop_var: bool, recursive: bool, span: Span) {
        self.start_for_loop_with_layout(with_loop_var, recursive, span, None);
    }

    fn start_for_loop_with_layout(
        &mut self,
        with_loop_var: bool,
        recursive: bool,
        span: Span,
        repeat_start_tag_span: Option<Span>,
    ) {
        let mut flags = 0;
        if with_loop_var {
            flags |= LOOP_FLAG_WITH_LOOP_VAR;
        }
        if recursive {
            flags |= LOOP_FLAG_RECURSIVE;
        }
        self.add(Instruction::PushLoop(flags, span));
        let instr = self.add(Instruction::Iterate(!0, span));
        if let Some(start_tag_span) = repeat_start_tag_span {
            self.add(Instruction::JinjaLayoutLoopIterationStart(start_tag_span));
        }
        self.pending_block.push(PendingBlock::Loop {
            iter_instr: instr,
            jump_instrs: Vec::new(),
        });
    }

    /// Ends the open for loop
    pub fn end_for_loop(&mut self, push_did_not_iterate: bool, span: Span) {
        self.end_for_loop_with_layout(push_did_not_iterate, span, None);
    }

    fn end_for_loop_with_layout(
        &mut self,
        push_did_not_iterate: bool,
        span: Span,
        skipped_end_tag_span: Option<Span>,
    ) {
        if let Some(PendingBlock::Loop {
            iter_instr,
            jump_instrs,
        }) = self.pending_block.pop()
        {
            self.add(Instruction::Jump(iter_instr, span));
            let loop_end = self.next_instruction();
            if let Some(end_tag_span) = skipped_end_tag_span {
                self.add(Instruction::JinjaLayoutEventIfLoopDidNotIterate(
                    JinjaLayoutEventKind::BlockEnd,
                    end_tag_span,
                ));
            }
            if push_did_not_iterate {
                self.add(Instruction::PushDidNotIterate);
            };
            self.add(Instruction::PopFrame);
            for instr in jump_instrs.into_iter().chain(Some(iter_instr)) {
                match self.instructions.get_mut(instr) {
                    Some(Instruction::Iterate(ref mut jump_target, _))
                    | Some(Instruction::Jump(ref mut jump_target, _)) => {
                        *jump_target = loop_end;
                    }
                    _ => unreachable!(),
                }
            }
        } else {
            unreachable!()
        }
    }

    /// Begins an if conditional
    pub fn start_if(&mut self, span: Span) {
        let jump_instr = self.add(Instruction::JumpIfFalse(!0, span));
        self.pending_block.push(PendingBlock::Branch { jump_instr });
    }

    /// Begins an else conditional
    pub fn start_else(&mut self, span: Span) {
        let jump_instr = self.add(Instruction::Jump(!0, span));
        self.end_condition(jump_instr + 1);
        self.pending_block.push(PendingBlock::Branch { jump_instr });
    }

    /// Closes the current if block.
    pub fn end_if(&mut self) {
        self.end_condition(self.next_instruction());
    }

    /// Starts a short-circuited bool block.
    pub fn start_sc_bool(&mut self) {
        self.pending_block.push(PendingBlock::ScBool {
            jump_instrs: Vec::new(),
        });
    }

    /// Emits a short-circuited bool operator.
    pub fn sc_bool(&mut self, and: bool, span: Span, type_constraints: Vec<TypeConstraint>) {
        match self.profile {
            CodeGenerationProfile::Render => {
                if let Some(PendingBlock::ScBool {
                    ref mut jump_instrs,
                }) = self.pending_block.last_mut()
                {
                    if and {
                        jump_instrs.push(
                            self.instructions
                                .add(Instruction::JumpIfFalseOrPop(!0, span)),
                        );
                        for type_constraint in type_constraints {
                            self.add(Instruction::TypeConstraint(type_constraint, true, span));
                        }
                    } else {
                        jump_instrs.push(
                            self.instructions
                                .add(Instruction::JumpIfTrueOrPop(!0, span)),
                        );
                        if type_constraints.len() == 1 {
                            // if there is only one type constraint, we can just add it to the else block
                            // if there is more than one, we don't have constraints
                            self.add(Instruction::TypeConstraint(
                                type_constraints[0].clone(),
                                false,
                                span,
                            ));
                        }
                    }
                } else {
                    unreachable!();
                }
            }
            CodeGenerationProfile::TypeCheck(_, _) => {
                if and {
                    self.instructions
                        .add(Instruction::StoreLocal("_internal_tmp", span));
                    self.instructions
                        .add(Instruction::Lookup("_internal_tmp", span));

                    self.start_sc_bool();

                    if let Some(PendingBlock::ScBool {
                        ref mut jump_instrs,
                    }) = self.pending_block.last_mut()
                    {
                        jump_instrs.push(self.instructions.add(Instruction::JumpIfTrue(!0, span)));
                    } else {
                        unreachable!();
                    }
                    if type_constraints.len() == 1 {
                        self.add(Instruction::TypeConstraint(
                            type_constraints[0].clone(),
                            false,
                            span,
                        ));
                    }

                    self.instructions
                        .add(Instruction::Lookup("_internal_tmp", span));
                    if let Some(PendingBlock::ScBool {
                        ref mut jump_instrs,
                    }) = self.pending_block.first_mut()
                    {
                        jump_instrs.push(self.instructions.add(Instruction::Jump(!0, span)));
                    }

                    self.end_sc_bool();
                    if type_constraints.len() == 1 {
                        let mut inverted = type_constraints[0].clone();
                        inverted.operation = inverted.operation.not();
                        self.add(Instruction::TypeConstraint(inverted, true, span));
                    }
                } else {
                    self.instructions
                        .add(Instruction::StoreLocal("_internal_tmp", span));
                    self.instructions
                        .add(Instruction::Lookup("_internal_tmp", span));

                    self.start_sc_bool();

                    if let Some(PendingBlock::ScBool {
                        ref mut jump_instrs,
                    }) = self.pending_block.last_mut()
                    {
                        jump_instrs.push(self.instructions.add(Instruction::JumpIfFalse(!0, span)));
                    } else {
                        unreachable!();
                    }
                    if type_constraints.len() == 1 {
                        self.add(Instruction::TypeConstraint(
                            type_constraints[0].clone(),
                            true,
                            span,
                        ));
                    }

                    self.instructions
                        .add(Instruction::Lookup("_internal_tmp", span));
                    if let Some(PendingBlock::ScBool {
                        ref mut jump_instrs,
                    }) = self.pending_block.first_mut()
                    {
                        jump_instrs.push(self.instructions.add(Instruction::Jump(!0, span)));
                    }

                    self.end_sc_bool();
                    if type_constraints.len() == 1 {
                        let mut inverted = type_constraints[0].clone();
                        inverted.operation = inverted.operation.not();
                        self.add(Instruction::TypeConstraint(inverted, false, span));
                    }
                }
            }
        }
    }

    /// Ends a short-circuited bool block.
    pub fn end_sc_bool(&mut self) {
        let end = self.next_instruction();
        if let Some(PendingBlock::ScBool { jump_instrs }) = self.pending_block.pop() {
            for instr in jump_instrs {
                match self.instructions.get_mut(instr) {
                    Some(Instruction::JumpIfFalseOrPop(ref mut target, _))
                    | Some(Instruction::JumpIfTrueOrPop(ref mut target, _))
                    | Some(Instruction::Jump(ref mut target, _))
                    | Some(Instruction::JumpIfFalse(ref mut target, _))
                    | Some(Instruction::JumpIfTrue(ref mut target, _)) => {
                        *target = end;
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    fn end_condition(&mut self, new_jump_instr: usize) {
        match self.pending_block.pop() {
            Some(PendingBlock::Branch { jump_instr }) => {
                match self.instructions.get_mut(jump_instr) {
                    Some(Instruction::JumpIfFalse(ref mut target, _))
                    | Some(Instruction::Jump(ref mut target, _)) => {
                        *target = new_jump_instr;
                    }
                    _ => {}
                }
            }
            _ => unreachable!(),
        }
    }

    /// Compiles a statement.
    pub fn compile_stmt(&mut self, stmt: &ast::Stmt<'source>) -> Result<(), crate::Error> {
        match stmt {
            ast::Stmt::Template(t) => {
                self.set_line_from_span(t.span());
                for node in &t.children {
                    self.compile_stmt(node)?;
                }
            }
            ast::Stmt::EmitExpr(expr) => {
                self.compile_emit_expr(expr)?;
            }
            ast::Stmt::EmitRaw(raw) => {
                self.set_line_from_span(raw.span());
                self.add(Instruction::EmitRaw(raw.raw, raw.span()));
                self.raw_template_bytes += raw.raw.len();
            }
            ast::Stmt::ForLoop(for_loop) => {
                self.compile_for_loop(for_loop)?;
            }
            ast::Stmt::IfCond(if_cond) => {
                self.compile_if_stmt(if_cond)?;
            }
            ast::Stmt::WithBlock(with_block) => {
                self.set_line_from_span(with_block.span());
                self.add(Instruction::PushWith(with_block.span()));
                for (target, expr) in &with_block.assignments {
                    self.compile_expr(expr)?;
                    self.compile_assignment(target)?;
                }
                for node in &with_block.body {
                    self.compile_stmt(node)?;
                }
                self.add(Instruction::PopFrame);
            }
            ast::Stmt::Set(set) => {
                self.set_line_from_span(set.span());
                let span = set.span();
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::BlockStandalone,
                    span,
                ));
                self.add(Instruction::MacroStart(
                    span.start_line,
                    span.start_col,
                    span.start_offset,
                ));

                self.compile_expr(&set.expr)?;
                self.compile_assignment(&set.target)?;

                self.add(Instruction::MacroStop(
                    span.end_line,
                    span.end_col,
                    span.end_offset,
                ));
            }
            ast::Stmt::SetBlock(set_block) => {
                self.set_line_from_span(set_block.span());
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::BlockStart,
                    set_block.start_tag_span,
                ));
                self.add(Instruction::BeginCapture(CaptureMode::Capture));
                for node in &set_block.body {
                    self.compile_stmt(node)?;
                }
                self.add(Instruction::EndCapture);
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::BlockEnd,
                    set_block.end_tag_span,
                ));
                if let Some(ref filter) = set_block.filter {
                    self.compile_expr(filter)?;
                }
                self.compile_assignment(&set_block.target)?;
            }
            ast::Stmt::AutoEscape(auto_escape) => {
                self.set_line_from_span(auto_escape.span());
                self.compile_expr(&auto_escape.enabled)?;
                self.add(Instruction::PushAutoEscape(auto_escape.span()));
                for node in &auto_escape.body {
                    self.compile_stmt(node)?;
                }
                self.add(Instruction::PopAutoEscape);
            }
            ast::Stmt::FilterBlock(filter_block) => {
                self.set_line_from_span(filter_block.span());
                self.add(Instruction::BeginCapture(CaptureMode::Capture));
                for node in &filter_block.body {
                    self.compile_stmt(node)?;
                }
                self.add(Instruction::EndCapture);
                self.compile_expr(&filter_block.filter)?;
                self.add(Instruction::Emit(filter_block.span()));
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::Block(block) => {
                self.compile_block(block)?;
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::Import(import) => {
                self.add(Instruction::BeginCapture(CaptureMode::Discard));
                self.add(Instruction::PushWith(import.span()));
                self.compile_expr(&import.expr)?;
                self.add_with_span(Instruction::Include(false, import.span()), import.span());
                self.add(Instruction::ExportLocals);
                self.add(Instruction::PopFrame);
                self.compile_assignment(&import.name)?;
                self.add(Instruction::EndCapture);
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::FromImport(from_import) => {
                self.add(Instruction::BeginCapture(CaptureMode::Discard));
                self.add(Instruction::PushWith(from_import.span()));
                self.compile_expr(&from_import.expr)?;
                self.add_with_span(
                    Instruction::Include(false, from_import.span()),
                    from_import.span(),
                );
                for (name, _) in &from_import.names {
                    self.compile_expr(name)?;
                }
                self.add(Instruction::PopFrame);
                for (name, alias) in from_import.names.iter().rev() {
                    self.compile_assignment(alias.as_ref().unwrap_or(name))?;
                }
                self.add(Instruction::EndCapture);
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::Extends(extends) => {
                self.set_line_from_span(extends.span());
                self.compile_expr(&extends.name)?;
                self.add_with_span(Instruction::LoadBlocks(extends.span()), extends.span());
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::Include(include) => {
                self.set_line_from_span(include.span());
                self.compile_expr(&include.name)?;
                self.add_with_span(
                    Instruction::Include(include.ignore_missing, include.span()),
                    include.span(),
                );
            }
            #[cfg(feature = "macros")]
            ast::Stmt::Macro(macro_decl) => {
                self.compile_macro(&macro_decl.0)?;
            }
            #[cfg(feature = "macros")]
            ast::Stmt::CallBlock(call_block) => {
                self.compile_call_block(call_block)?;
            }
            #[cfg(feature = "loop_controls")]
            ast::Stmt::Continue(cont) => {
                self.set_line_from_span(cont.span());
                for pending_block in self.pending_block.iter().rev() {
                    if let PendingBlock::Loop { iter_instr, .. } = pending_block {
                        self.add(Instruction::Jump(*iter_instr, cont.span));
                        break;
                    }
                }
            }
            #[cfg(feature = "loop_controls")]
            ast::Stmt::Break(brk) => {
                match &self.profile {
                    CodeGenerationProfile::Render => {
                        self.set_line_from_span(brk.span());
                        let instr = self.add(Instruction::Jump(0, brk.span));
                        for pending_block in self.pending_block.iter_mut().rev() {
                            if let PendingBlock::Loop {
                                ref mut jump_instrs,
                                ..
                            } = pending_block
                            {
                                jump_instrs.push(instr);
                                break;
                            }
                        }
                    }
                    CodeGenerationProfile::TypeCheck(_, _) => {
                        // do nothing
                    }
                }
            }
            ast::Stmt::Do(do_tag) => {
                let span = do_tag.span();
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::BlockStandalone,
                    span,
                ));
                self.add(Instruction::MacroStart(
                    span.start_line,
                    span.start_col,
                    span.start_offset,
                ));
                self.compile_do(do_tag)?;
                self.add(Instruction::MacroStop(
                    span.end_line,
                    span.end_col,
                    span.end_offset,
                ));
            }
            ast::Stmt::Comment(comment) => {
                let span = comment.span();
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::Comment,
                    span,
                ));
                self.add(Instruction::MacroStart(
                    span.start_line,
                    span.start_col,
                    span.start_offset,
                ));
                self.add(Instruction::MacroStop(
                    span.end_line,
                    span.end_col,
                    span.end_offset,
                ));
            }
        }
        Ok(())
    }

    #[cfg(feature = "multi_template")]
    fn compile_block(
        &mut self,
        block: &ast::Spanned<ast::Block<'source>>,
    ) -> Result<(), crate::Error> {
        self.set_line_from_span(block.span());
        let mut sub = self.new_subgenerator();
        for node in &block.body {
            sub.compile_stmt(node)?;
        }
        let instructions = self.finish_subgenerator(sub);
        self.blocks.insert(block.name, instructions);
        self.add(Instruction::CallBlock(block.name));
        Ok(())
    }

    #[cfg(feature = "macros")]
    fn compile_macro_expression(
        &mut self,
        macro_decl: &ast::Spanned<ast::Macro<'source>>,
    ) -> Result<(), crate::Error> {
        use crate::compiler::instructions::MACRO_CALLER;
        self.set_line_from_span(macro_decl.span());
        let instr = self.add(Instruction::Jump(!0, macro_decl.span()));
        self.add(Instruction::MacroName(macro_decl.name, macro_decl.span()));

        // dbt function parameters support lateral variables, e.g.
        // {% macro foo(a, b=a+1, c=b+1) %}
        // So we have to evaluate the defaults from left to right
        let mut defaults_iter = macro_decl.defaults.iter();
        for (i, arg) in macro_decl.args.iter().enumerate() {
            if i >= macro_decl.args.len() - macro_decl.defaults.len() {
                let default = defaults_iter.next().unwrap();
                match &self.profile {
                    CodeGenerationProfile::Render => {
                        self.add(Instruction::DupTop);
                        self.add(Instruction::IsUndefined);
                        self.start_if(macro_decl.span());
                        self.add(Instruction::DiscardTop);
                        self.compile_expr(default)?;
                        self.end_if();
                    }
                    CodeGenerationProfile::TypeCheck(
                        function_registry,
                        typecheck_resolved_context,
                    ) => {
                        let mut attempts = Vec::new();
                        let resolved_signature = macro_namespace_template_resolver(
                            typecheck_resolved_context,
                            function_registry.clone(),
                            macro_decl.name,
                            &mut attempts,
                        )
                        .and_then(|macro_qualified_name| {
                            function_registry.get(&macro_qualified_name).cloned()
                        });
                        match resolved_signature.as_ref().and_then(|signature| {
                            signature.downcast_ref::<UserDefinedFunctionType>()
                        }) {
                            Some(udf) => {
                                let type_ = udf
                                    .args
                                    .get(i)
                                    .map(|arg| arg.type_.get_non_optional_type())
                                    .unwrap_or(Type::Any { hard: false });
                                // the parameter has default value, so we need to exclude the none from arg type and check with the default
                                self.add(Instruction::LoadType(Value::from_object(type_)));
                                self.compile_expr(default)?;
                                self.add(Instruction::UnionType);
                            }
                            None => {
                                // No statically-known signature for this macro
                                // (e.g. it has no docstring `funcsign` -- the
                                // common case for most macros -- or, until
                                // recently, `macro_namespace_template_resolver`
                                // couldn't resolve a package macro's own
                                // qualified name at all when typechecking it in
                                // isolation without a `target_package_name`).
                                // Fall back to `Any` for the declared type, but
                                // still compile the default expression and
                                // union it in -- omitting either one leaves
                                // this argument's assignment with nothing to
                                // pop off the stack (a stack underflow once
                                // typechecking reaches it), and skipping
                                // `compile_expr(default)` would also silently
                                // stop the type checker from ever visiting
                                // calls made inside the default expression
                                // itself.
                                self.add(Instruction::LoadType(Value::from_object(Type::Any {
                                    hard: false,
                                })));
                                self.compile_expr(default)?;
                                self.add(Instruction::UnionType);
                            }
                        }
                    }
                }
            } else if let CodeGenerationProfile::TypeCheck(
                function_registry,
                typecheck_resolved_context,
            ) = &self.profile
            {
                let mut attempts = Vec::new();
                if let Some(macro_qualified_name) = macro_namespace_template_resolver(
                    typecheck_resolved_context,
                    function_registry.clone(),
                    macro_decl.name,
                    &mut attempts,
                ) {
                    if let Some(signature) = function_registry.get(&macro_qualified_name) {
                        if let Some(udf) = signature.downcast_ref::<UserDefinedFunctionType>() {
                            let type_ = udf
                                .args
                                .get(i)
                                .map(|a| a.type_.clone())
                                .unwrap_or(Type::Any { hard: false });
                            self.add(Instruction::LoadType(Value::from_object(type_)));
                        } else {
                            self.add(Instruction::LoadType(Value::from_object(Type::Any {
                                hard: false,
                            })));
                        }
                    } else {
                        panic!("Function signature not found for macro {}", macro_decl.name);
                    }
                } else {
                    self.add(Instruction::LoadType(Value::from_object(Type::Any {
                        hard: false,
                    })));
                }
            }
            self.compile_assignment(arg)?;
        }
        let span = macro_decl.span();

        for node in &macro_decl.body {
            self.compile_stmt(node)?;
        }
        self.add(Instruction::Return {
            explicit: false,
            arg_count: None,
        });
        let mut undeclared = crate::compiler::meta::find_macro_closure(macro_decl);
        let caller_reference = undeclared.remove("caller");
        let macro_instr = self.next_instruction();
        for name in &undeclared {
            self.add(Instruction::Enclose(name));
        }
        self.add(Instruction::GetClosure);
        self.add(Instruction::LoadConst(Value::from_object(
            macro_decl
                .args
                .iter()
                .map(|x| match x {
                    ast::Expr::Var(var) => Value::from(var.id),
                    _ => unreachable!(),
                })
                .collect::<Vec<Value>>(),
        )));
        let mut flags = 0;
        if caller_reference {
            flags |= MACRO_CALLER;
        }
        self.add(Instruction::MacroStart(
            span.start_line,
            span.start_col,
            span.start_offset,
        ));

        self.add(Instruction::BuildMacro(
            macro_decl.name,
            instr + 1,
            flags,
            span,
        ));
        self.add(Instruction::MacroStop(
            span.end_line,
            span.end_col,
            span.end_offset,
        ));

        if let Some(Instruction::Jump(ref mut target, _)) = self.instructions.get_mut(instr) {
            *target = macro_instr;
        } else {
            unreachable!();
        }
        Ok(())
    }

    #[cfg(feature = "macros")]
    fn compile_macro(
        &mut self,
        macro_decl: &ast::Spanned<ast::Macro<'source>>,
    ) -> Result<(), crate::Error> {
        self.compile_macro_expression(macro_decl)?;
        self.add(Instruction::StoreLocal(macro_decl.name, macro_decl.span()));
        Ok(())
    }

    #[cfg(feature = "macros")]
    fn compile_call_block(
        &mut self,
        call_block: &ast::Spanned<ast::CallBlock<'source>>,
    ) -> Result<(), crate::Error> {
        self.compile_call(&call_block.call, Some(&call_block.macro_decl))?;
        self.add(Instruction::Emit(call_block.span()));
        Ok(())
    }

    fn compile_do(&mut self, do_tag: &ast::Spanned<ast::Do<'source>>) -> Result<(), crate::Error> {
        if let ast::Expr::Call(call) = &do_tag.expr {
            if let ast::CallType::Function(name) = call.identify_call() {
                if name == "return" {
                    let arg_count = self.compile_call_args(&call.args, 0, None, do_tag.span())?;
                    self.add(Instruction::Return {
                        explicit: true,
                        arg_count: Some(arg_count),
                    });
                    return Ok(());
                }
            }
        }
        self.compile_expr(&do_tag.expr)?;
        self.add(Instruction::DiscardTop);
        Ok(())
    }

    fn compile_if_stmt(
        &mut self,
        if_cond: &ast::Spanned<ast::IfCond<'source>>,
    ) -> Result<(), crate::Error> {
        self.set_line_from_span(if_cond.span());
        let span = if_cond.span();
        self.add(Instruction::JinjaLayoutEvent(
            if_cond.start_tag_kind,
            if_cond.start_tag_span,
        ));
        self.add(Instruction::MacroStart(
            span.start_line,
            span.start_col,
            span.start_offset,
        ));

        self.compile_expr(&if_cond.expr)?;
        let type_constraints = self.get_type_constraints(&if_cond.expr);
        self.start_if(span);
        for type_constraint in &type_constraints {
            self.add(Instruction::TypeConstraint(
                type_constraint.clone(),
                true,
                span,
            ));
        }
        for node in &if_cond.true_body {
            self.compile_stmt(node)?;
        }
        if !if_cond.false_body.is_empty()
            || matches!(self.profile, CodeGenerationProfile::TypeCheck(_, _))
        {
            if let Some(else_tag_span) = if_cond.else_tag_span {
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::BlockMid,
                    else_tag_span,
                ));
            } else if let Some(ast::Stmt::IfCond(nested_if_cond)) = if_cond.false_body.first() {
                if nested_if_cond.start_tag_kind == JinjaLayoutEventKind::BlockMid {
                    self.add(Instruction::JinjaLayoutEvent(
                        JinjaLayoutEventKind::BlockMid,
                        nested_if_cond.start_tag_span,
                    ));
                }
            }
            self.start_else(span);
            if let Some(else_tag_span) = if_cond.else_tag_span {
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::BlockMid,
                    else_tag_span,
                ));
            }
            if type_constraints.len() == 1 {
                // if there is only one type constraint, we can just add it to the else block
                // if there is more than one, we don't have constraints
                let mut inverted = type_constraints[0].clone();
                inverted.operation = inverted.operation.not();
                self.add(Instruction::TypeConstraint(inverted, false, span));
            }
            if !if_cond.false_body.is_empty() {
                for node in &if_cond.false_body {
                    self.compile_stmt(node)?;
                }
            }
        }
        self.end_if();
        self.add(Instruction::JinjaLayoutEvent(
            JinjaLayoutEventKind::BlockEnd,
            if_cond.end_tag_span,
        ));
        self.add(Instruction::MacroStop(
            span.end_line,
            span.end_col,
            span.end_offset,
        ));
        Ok(())
    }

    fn compile_emit_expr(
        &mut self,
        expr: &ast::Spanned<ast::EmitExpr<'source>>,
    ) -> Result<(), crate::Error> {
        self.set_line_from_span(expr.span());
        let span = expr.span();
        self.add(Instruction::JinjaLayoutEvent(
            JinjaLayoutEventKind::Variable,
            span,
        ));
        self.add(Instruction::MacroStart(
            span.start_line,
            span.start_col,
            span.start_offset,
        ));

        if let ast::Expr::Call(call) = &expr.expr {
            match call.identify_call() {
                ast::CallType::Function(name) => {
                    if name == "return" {
                        let arg_count = self.compile_call_args(&call.args, 0, None, span)?;
                        self.add(Instruction::MacroStop(
                            span.end_line,
                            span.end_col,
                            span.end_offset,
                        ));

                        self.add(Instruction::Return {
                            explicit: true,
                            arg_count: Some(arg_count),
                        });
                        return Ok(());
                    } else if name == "super" && call.args.is_empty() {
                        self.add_with_span(Instruction::FastSuper(call.span()), call.span());
                        return Ok(());
                    } else if name == "loop" && call.args.len() == 1 {
                        self.compile_call_args(
                            std::slice::from_ref(&call.args[0]),
                            0,
                            None,
                            call.span(),
                        )?;
                        self.add(Instruction::FastRecurse(call.span()));
                        return Ok(());
                    }
                }
                #[cfg(feature = "multi_template")]
                ast::CallType::Block(name) => {
                    self.add(Instruction::CallBlock(name));
                    return Ok(());
                }
                _ => {}
            }
        }
        self.compile_expr(&expr.expr)?;
        self.add(Instruction::Emit(expr.span()));
        self.add(Instruction::MacroStop(
            span.end_line,
            span.end_col,
            span.end_offset,
        ));
        Ok(())
    }

    fn compile_for_loop(
        &mut self,
        for_loop: &ast::Spanned<ast::ForLoop<'source>>,
    ) -> Result<(), crate::Error> {
        self.set_line_from_span(for_loop.span());
        let span = for_loop.span();
        self.add(Instruction::JinjaLayoutEvent(
            JinjaLayoutEventKind::BlockStart,
            for_loop.start_tag_span,
        ));
        self.add(Instruction::MacroStart(
            span.start_line,
            span.start_col,
            span.start_offset,
        ));

        // filter expressions work like a nested for loop without
        // the special loop variable. in one loop, the condition is checked and
        // passing items accumulated into a list. in the second, that list is
        // iterated over normally
        if let Some(ref filter_expr) = for_loop.filter_expr {
            self.add(Instruction::LoadConst(Value::from(0usize)));
            self.compile_expr(&for_loop.iter)?;
            self.start_for_loop_with_layout(false, false, span, Some(for_loop.start_tag_span));
            self.add(Instruction::DupTop);
            self.compile_assignment(&for_loop.target)?;
            self.compile_expr(filter_expr)?;
            self.start_if(span);
            self.add(Instruction::Swap);
            self.add(Instruction::LoadConst(Value::from(1usize)));
            self.add(Instruction::Add(span));
            self.start_else(span);
            self.add(Instruction::DiscardTop);
            self.end_if();
            self.end_for_loop_with_layout(false, span, None);
            self.add(Instruction::BuildList(None, span));
        } else {
            self.compile_expr(&for_loop.iter)?;
        }

        self.start_for_loop_with_layout(
            true,
            for_loop.recursive,
            span,
            Some(for_loop.start_tag_span),
        );
        self.compile_assignment(&for_loop.target)?;
        for node in &for_loop.body {
            self.compile_stmt(node)?;
        }
        self.add(Instruction::JinjaLayoutEvent(
            JinjaLayoutEventKind::BlockEnd,
            for_loop.end_tag_span,
        ));
        self.end_for_loop_with_layout(
            !for_loop.else_body.is_empty(),
            span,
            for_loop
                .else_body
                .is_empty()
                .then_some(for_loop.end_tag_span),
        );
        if !for_loop.else_body.is_empty() {
            self.start_if(span);
            if let Some(else_tag_span) = for_loop.else_tag_span {
                self.add(Instruction::JinjaLayoutEvent(
                    JinjaLayoutEventKind::BlockMid,
                    else_tag_span,
                ));
            }
            for node in &for_loop.else_body {
                self.compile_stmt(node)?;
            }
            self.end_if();
            self.add(Instruction::JinjaLayoutEvent(
                JinjaLayoutEventKind::BlockEnd,
                for_loop.end_tag_span,
            ));
        };
        self.add(Instruction::MacroStop(
            span.end_line,
            span.end_col,
            span.end_offset,
        ));
        Ok(())
    }

    /// Compiles an assignment expression.
    pub fn compile_assignment(&mut self, expr: &ast::Expr<'source>) -> Result<(), crate::Error> {
        match expr {
            ast::Expr::Var(var) => {
                self.add(Instruction::StoreLocal(var.id, var.span()));
            }
            ast::Expr::List(list) => {
                self.push_span(list.span());
                self.add(Instruction::UnpackList(list.items.len(), list.span()));
                for expr in &list.items {
                    self.compile_assignment(expr)?;
                }
                self.pop_span();
            }
            ast::Expr::Tuple(tuple) => {
                self.push_span(tuple.span());
                self.add(Instruction::UnpackList(tuple.items.len(), tuple.span()));
                for expr in &tuple.items {
                    self.compile_assignment(expr)?;
                }
                self.pop_span();
            }
            ast::Expr::GetAttr(attr) => {
                self.push_span(attr.span());
                self.compile_expr(&attr.expr)?;
                self.add(Instruction::SetAttr(attr.name, attr.span()));
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Compiles an expression.
    pub fn compile_expr(&mut self, expr: &ast::Expr<'source>) -> Result<(), crate::Error> {
        match expr {
            ast::Expr::Var(v) => {
                self.set_line_from_span(v.span());
                self.add(Instruction::Lookup(v.id, v.span()));
            }
            ast::Expr::Const(v) => {
                self.set_line_from_span(v.span());
                self.add(Instruction::LoadConst(v.value.clone()));
            }
            ast::Expr::Slice(s) => {
                self.push_span(s.span());
                self.compile_expr(&s.expr)?;
                if let Some(ref start) = s.start {
                    self.compile_expr(start)?;
                } else {
                    self.add(Instruction::LoadConst(Value::from(())));
                }
                if let Some(ref stop) = s.stop {
                    self.compile_expr(stop)?;
                } else {
                    self.add(Instruction::LoadConst(Value::from(())));
                }
                if let Some(ref step) = s.step {
                    self.compile_expr(step)?;
                } else {
                    self.add(Instruction::LoadConst(Value::from(1)));
                }
                self.add(Instruction::Slice(s.span()));
                self.pop_span();
            }
            ast::Expr::UnaryOp(c) => {
                self.set_line_from_span(c.span());
                match c.op {
                    ast::UnaryOpKind::Not => {
                        self.compile_expr(&c.expr)?;
                        self.add(Instruction::Not(c.span()));
                    }
                    ast::UnaryOpKind::Neg => {
                        // common case: negative numbers.  In that case we
                        // directly negate them if this is possible without
                        // an error.
                        if let ast::Expr::Const(ref c) = c.expr {
                            if let Ok(negated) = neg(&c.value) {
                                self.add(Instruction::LoadConst(negated));
                                return Ok(());
                            }
                        }
                        self.compile_expr(&c.expr)?;
                        self.add_with_span(Instruction::Neg(c.span()), c.span());
                    }
                    ast::UnaryOpKind::Pos => {
                        // unary plus validates numeric type at runtime
                        // For non-numeric types, it will fail at runtime
                        self.compile_expr(&c.expr)?;
                        self.add_with_span(Instruction::Pos(c.span()), c.span());
                    }
                }
            }

            ast::Expr::BinOp(c) => {
                self.compile_bin_op(c)?;
            }
            ast::Expr::IfExpr(i) => {
                self.set_line_from_span(i.span());
                self.compile_expr(&i.test_expr)?;
                let type_constraints = self.get_type_constraints(&i.test_expr);
                self.start_if(i.span());
                for type_constraint in &type_constraints {
                    self.add(Instruction::TypeConstraint(
                        type_constraint.clone(),
                        true,
                        i.span(),
                    ));
                }
                self.compile_expr(&i.true_expr)?;
                self.start_else(i.span);
                if type_constraints.len() == 1 {
                    // if there is only one type constraint, we can just add it to the else block
                    // if there is more than one, we don't have constraints
                    let mut inverted = type_constraints[0].clone();
                    inverted.operation = inverted.operation.not();
                    self.add(Instruction::TypeConstraint(inverted, false, i.span()));
                }
                if let Some(ref false_expr) = i.false_expr {
                    self.compile_expr(false_expr)?;
                } else {
                    self.add(Instruction::LoadConst(Value::UNDEFINED));
                }
                self.end_if();
            }
            ast::Expr::Filter(f) => {
                self.push_span(f.span());
                if let Some(ref expr) = f.expr {
                    self.compile_expr(expr)?;
                }
                let arg_count = self.compile_call_args(&f.args, 1, None, f.span())?;
                let local_id = get_local_id(&mut self.filter_local_ids, f.name);
                self.add(Instruction::ApplyFilter(
                    f.name,
                    arg_count,
                    local_id,
                    f.span(),
                ));
                self.pop_span();
            }
            ast::Expr::Test(f) => {
                self.push_span(f.span());
                self.compile_expr(&f.expr)?;
                let arg_count = self.compile_call_args(&f.args, 1, None, f.span())?;
                let local_id = get_local_id(&mut self.test_local_ids, f.name);
                self.add(Instruction::PerformTest(
                    f.name,
                    arg_count,
                    local_id,
                    f.span(),
                ));
                self.pop_span();
            }
            ast::Expr::GetAttr(g) => {
                self.push_span(g.span());
                self.compile_expr(&g.expr)?;
                self.add(Instruction::GetAttr(g.name, g.span()));
                self.pop_span();
            }
            ast::Expr::GetItem(g) => {
                self.push_span(g.span());
                self.compile_expr(&g.expr)?;
                self.compile_expr(&g.subscript_expr)?;
                self.add(Instruction::GetItem(g.span()));
                self.pop_span();
            }
            ast::Expr::Call(c) => {
                self.compile_call(c, None)?;
            }
            ast::Expr::List(l) => {
                self.set_line_from_span(l.span());
                for item in &l.items {
                    self.compile_expr(item)?;
                }
                self.add(Instruction::BuildList(Some(l.items.len()), l.span()));
            }
            ast::Expr::Map(m) => {
                self.set_line_from_span(m.span());
                assert_eq!(m.keys.len(), m.values.len());
                for (key, value) in m.keys.iter().zip(m.values.iter()) {
                    self.compile_expr(key)?;
                    self.compile_expr(value)?;
                }
                self.add(Instruction::BuildMap(m.keys.len(), m.span()));
            }
            ast::Expr::Tuple(t) => {
                self.set_line_from_span(t.span());
                for item in &t.items {
                    self.compile_expr(item)?;
                }
                self.add(Instruction::BuildTuple(Some(t.items.len()), t.span()));
            }
        }
        Ok(())
    }

    fn compile_call(
        &mut self,
        c: &ast::Spanned<ast::Call<'source>>,
        caller: Option<&Caller<'source>>,
    ) -> Result<(), crate::Error> {
        let span = c.span();
        self.push_span(span);

        match c.identify_call() {
            ast::CallType::Function(name) => {
                let arg_count = self.compile_call_args(&c.args, 0, caller, span)?;
                let ref_or_source_span = if name == "ref" || name == "source" {
                    // Purely diagnostic: the span of the referenced literal, so errors can
                    // point at the name rather than the whole call.
                    //
                    // A zero-argument `ref()` / `source()` is invalid, but raising that is the
                    // function's job at call time (it already does — see `ResolveRefFunction`
                    // in dbt-jinja-utils), not codegen's. There is simply no argument here to
                    // take a span from, so don't index blindly: `&c.args[0]` and
                    // `c.args.last().unwrap()` both panicked, taking down compilation of every
                    // other node in the project along with the malformed one.
                    let arg = if name == "ref" {
                        c.args.first()
                    } else {
                        c.args.last()
                    };
                    match arg {
                        Some(ast::CallArg::Pos(ast::Expr::Const(c))) => Some(c.span().into()),
                        _ => None,
                    }
                } else {
                    None
                };
                let identifier_span = match get_identifier_span(&c.expr) {
                    Some(identifier_span) => identifier_span,
                    _ => span, // fallback to span
                };
                self.add(Instruction::CallFunction(
                    name,
                    arg_count,
                    identifier_span.into(),
                    span.into(),
                    ref_or_source_span,
                ));
            }
            #[cfg(feature = "multi_template")]
            ast::CallType::Block(name) => {
                self.add(Instruction::BeginCapture(CaptureMode::Capture));
                self.add(Instruction::CallBlock(name));
                self.add(Instruction::EndCapture);
            }
            ast::CallType::Method(expr, name) => {
                self.compile_expr(expr)?;
                let arg_count = self.compile_call_args(&c.args, 1, caller, span)?;
                let identifier_span = match get_identifier_span(&c.expr) {
                    Some(identifier_span) => identifier_span,
                    _ => span, // fallback to span
                };
                // Remember the receiver's name for plain variable receivers, so that
                // calling a method on an undefined receiver can report which variable
                // was undefined rather than just the method name. Only `Var` is
                // captured: it is the common case and the only receiver whose written
                // form is a single identifier, so there is no risk of naming a
                // sub-expression the author did not write.
                let receiver_name = match expr {
                    ast::Expr::Var(var) => Some(var.id),
                    _ => None,
                };
                self.add(Instruction::CallMethod(
                    name,
                    arg_count,
                    identifier_span.into(),
                    span.into(),
                    receiver_name,
                ));
            }
            ast::CallType::Object(expr) => {
                self.compile_expr(expr)?;
                let arg_count = self.compile_call_args(&c.args, 1, caller, span)?;
                self.add(Instruction::CallObject(arg_count, span));
            }
        };
        self.pop_span();
        Ok(())
    }

    fn compile_call_args(
        &mut self,
        args: &[ast::CallArg<'source>],
        extra_args: usize,
        caller: Option<&Caller<'source>>,
        span: Span,
    ) -> Result<Option<u16>, crate::Error> {
        let mut pending_args = extra_args;
        let mut num_args_batches = 0;
        let mut has_kwargs = caller.is_some();
        let mut static_kwargs = caller.is_none();

        for arg in args {
            match arg {
                ast::CallArg::Pos(expr) => {
                    self.compile_expr(expr)?;
                    pending_args += 1;
                }
                ast::CallArg::PosSplat(_expr)
                    if matches!(self.profile, CodeGenerationProfile::TypeCheck(_, _)) =>
                {
                    // Type check mode, we need to push a placeholder for the any type
                    self.add(Instruction::LoadType(Value::from_object(Type::Any {
                        hard: true,
                    })));
                    pending_args += 1;
                }
                ast::CallArg::PosSplat(expr) => {
                    if pending_args > 0 {
                        self.add(Instruction::BuildList(Some(pending_args), span));
                        pending_args = 0;
                        num_args_batches += 1;
                    }
                    self.compile_expr(expr)?;
                    num_args_batches += 1;
                }
                ast::CallArg::Kwarg(_, expr) => {
                    if !matches!(expr, ast::Expr::Const(_)) {
                        static_kwargs = false;
                    }
                    has_kwargs = true;
                }
                ast::CallArg::KwargSplat(_) => {
                    static_kwargs = false;
                    has_kwargs = true;
                }
            }
        }

        if has_kwargs {
            let mut pending_kwargs = 0;
            let mut num_kwargs_batches = 0;
            let mut collected_kwargs = ValueMap::new();
            for arg in args {
                match arg {
                    ast::CallArg::Kwarg(key, value) => {
                        if static_kwargs {
                            if let ast::Expr::Const(c) = value {
                                collected_kwargs.insert(Value::from(*key), c.value.clone());
                            } else {
                                unreachable!();
                            }
                        } else {
                            self.add(Instruction::LoadConst(Value::from(*key)));
                            self.compile_expr(value)?;
                            pending_kwargs += 1;
                        }
                    }
                    ast::CallArg::KwargSplat(expr) => {
                        if pending_kwargs > 0 {
                            self.add(Instruction::BuildKwargs(pending_kwargs));
                            num_kwargs_batches += 1;
                            pending_kwargs = 0;
                        }
                        self.compile_expr(expr)?;
                        num_kwargs_batches += 1;
                    }
                    ast::CallArg::Pos(_) | ast::CallArg::PosSplat(_) => {}
                }
            }

            if !collected_kwargs.is_empty() {
                self.add(Instruction::LoadConst(Kwargs::wrap(collected_kwargs)));
            } else {
                // The conditions above guarantee that if we collect static kwargs
                // we cannot enter this block (single kwargs batch, no caller).

                #[cfg(feature = "macros")]
                {
                    if let Some(caller) = caller {
                        self.add(Instruction::LoadConst(Value::from("caller")));
                        self.compile_macro_expression(caller)?;
                        pending_kwargs += 1
                    }
                }
                if num_kwargs_batches > 0 {
                    if pending_kwargs > 0 {
                        self.add(Instruction::BuildKwargs(pending_kwargs));
                        num_kwargs_batches += 1;
                    }
                    self.add(Instruction::MergeKwargs(num_kwargs_batches));
                } else {
                    self.add(Instruction::BuildKwargs(pending_kwargs));
                }
            }
            pending_args += 1;
        }

        if num_args_batches > 0 {
            if pending_args > 0 {
                self.add(Instruction::BuildList(Some(pending_args), span));
                num_args_batches += 1;
            }
            self.add(Instruction::UnpackLists(num_args_batches, span));
            Ok(None)
        } else {
            assert!(pending_args as u16 as usize == pending_args);
            Ok(Some(pending_args as u16))
        }
    }

    fn compile_bin_op(
        &mut self,
        c: &ast::Spanned<ast::BinOp<'source>>,
    ) -> Result<(), crate::Error> {
        let span = c.span();
        self.push_span(span);
        let instr = match c.op {
            ast::BinOpKind::Eq => Instruction::Eq(span),
            ast::BinOpKind::Ne => Instruction::Ne(span),
            ast::BinOpKind::Lt => Instruction::Lt(span),
            ast::BinOpKind::Lte => Instruction::Lte(span),
            ast::BinOpKind::Gt => Instruction::Gt(span),
            ast::BinOpKind::Gte => Instruction::Gte(span),
            ast::BinOpKind::ScAnd | ast::BinOpKind::ScOr => {
                self.start_sc_bool();
                self.compile_expr(&c.left)?;
                let mut type_constraints = self.get_type_constraints(&c.left);
                if matches!(c.op, ast::BinOpKind::ScAnd) {
                    // invert
                    type_constraints.iter_mut().for_each(|tc| {
                        tc.operation = tc.operation.not();
                    });
                }
                self.sc_bool(
                    matches!(c.op, ast::BinOpKind::ScAnd),
                    span,
                    type_constraints,
                );
                self.compile_expr(&c.right)?;
                self.end_sc_bool();
                self.pop_span();
                return Ok(());
            }
            ast::BinOpKind::Add => Instruction::Add(span),
            ast::BinOpKind::Sub => Instruction::Sub(span),
            ast::BinOpKind::Mul => Instruction::Mul(span),
            ast::BinOpKind::Div => Instruction::Div(span),
            ast::BinOpKind::FloorDiv => Instruction::IntDiv(span),
            ast::BinOpKind::Rem => Instruction::Rem(span),
            ast::BinOpKind::Pow => Instruction::Pow(span),
            ast::BinOpKind::Concat => Instruction::StringConcat(span),
            ast::BinOpKind::In => Instruction::In(span),
        };
        self.compile_expr(&c.left)?;
        self.compile_expr(&c.right)?;
        self.add(instr);
        self.pop_span();
        Ok(())
    }

    /// Returns the size hint for buffers.
    ///
    /// This is a proposal for the initial buffer size when rendering directly to a string.
    pub fn buffer_size_hint(&self) -> usize {
        // for now the assumption is made that twice the bytes of template code without
        // control structures, rounded up to the next power of two is a good default.  The
        // round to the next power of two is chosen because the underlying vector backing
        // strings prefers powers of two.
        (self.raw_template_bytes * 2).next_power_of_two()
    }

    /// Converts the compiler into the instructions.
    pub fn finish(
        self,
    ) -> (
        Instructions<'source>,
        BTreeMap<&'source str, Instructions<'source>>,
    ) {
        assert!(self.pending_block.is_empty());
        (self.instructions, self.blocks)
    }

    pub fn get_type_constraints(&self, expr: &ast::Expr<'source>) -> Vec<TypeConstraint> {
        if matches!(self.profile, CodeGenerationProfile::TypeCheck(_, _)) {
            get_type_constraints(expr).unwrap_or_default()
        } else {
            vec![]
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TypeConstraint {
    pub name: Variable,
    pub operation: TypeConstraintOperation,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum TypeConstraintOperation {
    NotNull(bool),
    Is(String, bool),
}

impl TypeConstraintOperation {
    pub fn not(&self) -> Self {
        match self {
            TypeConstraintOperation::NotNull(b) => TypeConstraintOperation::NotNull(!b),
            TypeConstraintOperation::Is(s, b) => TypeConstraintOperation::Is(s.clone(), !b),
        }
    }
}

fn get_type_constraints<'source>(expr: &ast::Expr<'source>) -> Result<Vec<TypeConstraint>, ()> {
    match expr {
        ast::Expr::Var(_) => Ok(vec![TypeConstraint {
            name: expr.try_into()?,
            operation: TypeConstraintOperation::NotNull(true),
        }]),
        ast::Expr::GetAttr(_) => Ok(vec![TypeConstraint {
            name: expr.try_into()?,
            operation: TypeConstraintOperation::NotNull(true),
        }]),
        ast::Expr::GetItem(_) => Ok(vec![TypeConstraint {
            name: expr.try_into()?,
            operation: TypeConstraintOperation::NotNull(true),
        }]),
        ast::Expr::Test(spanned) => {
            let test_name = if spanned.name.to_lowercase() == "sameas" {
                spanned.args.first().and_then(|arg| match arg {
                    ast::CallArg::Pos(expr) => {
                        if let ast::Expr::Const(c) = expr {
                            Some(c.value.to_string().to_lowercase())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
            } else {
                Some(spanned.name.to_string())
            };
            if let Some(test_name) = test_name {
                (&spanned.expr).try_into().map(|variable| {
                    vec![TypeConstraint {
                        name: variable,
                        operation: TypeConstraintOperation::Is(test_name, true),
                    }]
                })
            } else {
                Ok(vec![])
            }
        }
        ast::Expr::BinOp(bin_op) => match bin_op.op {
            ast::BinOpKind::ScAnd | ast::BinOpKind::ScOr => {
                let mut left_constraints = get_type_constraints(&bin_op.left)?;
                let right_constraints = get_type_constraints(&bin_op.right)?;
                left_constraints.extend(right_constraints);
                Ok(left_constraints)
            }
            _ => Ok(vec![]),
        },
        ast::Expr::UnaryOp(unary_op) if matches!(unary_op.op, ast::UnaryOpKind::Not) => {
            Ok(get_type_constraints(&unary_op.expr)?
                .into_iter()
                .map(|constraint| TypeConstraint {
                    name: constraint.name.clone(),
                    operation: constraint.operation.not(),
                })
                .collect())
        }
        ast::Expr::Call(_) => Ok(vec![TypeConstraint {
            name: "_internal_tmp".into(),
            operation: TypeConstraintOperation::NotNull(true),
        }]),
        ast::Expr::Filter(filter) => {
            if filter.name == "is_list" {
                if let Some(ref expr) = filter.expr {
                    if let Ok(variable) = expr.try_into() {
                        return Ok(vec![TypeConstraint {
                            name: variable,
                            operation: TypeConstraintOperation::Is("sequence".into(), true),
                        }]);
                    }
                }
            }
            Ok(vec![TypeConstraint {
                name: "_internal_tmp".into(),
                operation: TypeConstraintOperation::NotNull(true),
            }])
        }
        _ => Ok(vec![]),
    }
}

#[derive(Clone, Deserialize, Serialize, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Variable {
    String(String),
    GetAttr(Vec<Part>),
}

impl Variable {
    pub fn get_attribute(&self, name: &str) -> Variable {
        match self {
            Variable::String(base) => Variable::GetAttr(vec![
                Part::String(base.clone()),
                Part::String(name.to_string()),
            ]),
            Variable::GetAttr(items) => {
                let mut new_items = items.clone();
                new_items.push(Part::String(name.to_string()));
                Variable::GetAttr(new_items)
            }
        }
    }
    pub fn get_subscript(&self, index: &str) -> Variable {
        match self {
            Variable::String(base) => Variable::GetAttr(vec![
                Part::String(base.clone()),
                Part::Subscript(index.to_string()),
            ]),
            Variable::GetAttr(items) => {
                let mut new_items = items.clone();
                new_items.push(Part::Subscript(index.to_string()));
                Variable::GetAttr(new_items)
            }
        }
    }
}

impl<'source> TryFrom<&ast::Expr<'source>> for Variable {
    type Error = ();

    fn try_from(expr: &ast::Expr<'source>) -> Result<Self, Self::Error> {
        match expr {
            ast::Expr::Var(spanned) => Ok(Variable::String(spanned.id.to_string())),
            ast::Expr::GetAttr(spanned) => {
                let variable: Variable = (&spanned.expr).try_into()?;
                Ok(variable.get_attribute(spanned.name))
            }
            ast::Expr::GetItem(spanned) => {
                let variable: Variable = (&spanned.expr).try_into()?;
                let subscript = match &spanned.subscript_expr {
                    ast::Expr::Const(c) => c.value.to_string(),
                    _ => return Err(()),
                };
                Ok(variable.get_subscript(&subscript))
            }
            _ => Err(()),
        }
    }
}

impl From<String> for Variable {
    fn from(s: String) -> Self {
        Variable::String(s)
    }
}

impl From<&str> for Variable {
    fn from(s: &str) -> Self {
        Variable::String(s.to_string())
    }
}

impl From<&Variable> for Variable {
    fn from(v: &Variable) -> Self {
        v.clone()
    }
}

impl From<&String> for Variable {
    fn from(s: &String) -> Self {
        Variable::String(s.clone())
    }
}

impl From<&&str> for Variable {
    fn from(s: &&str) -> Self {
        Variable::String((*s).to_string())
    }
}
