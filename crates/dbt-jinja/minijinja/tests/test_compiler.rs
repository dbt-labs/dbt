#![cfg(feature = "unstable_machinery")]
use minijinja::compiler::codegen::CodeGenerationProfile;
use minijinja::machinery::{CodeGenerator, Instruction, Span};
use minijinja::value::Value;

#[test]
fn test_for_loop() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::Lookup("items", Span::default()));
    c.start_for_loop(true, false, Span::default());
    c.add(Instruction::Emit(Span::default()));
    c.end_for_loop(false, Span::default());
    c.add(Instruction::EmitRaw("!", Span::default()));

    insta::assert_debug_snapshot!(&c.finish());
}

#[test]
fn test_if_branches() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::Lookup("false", Span::default()));
    c.start_if(Span::default());
    c.add(Instruction::EmitRaw("nope1", Span::default()));
    c.start_else(Span::default());
    c.add(Instruction::Lookup("nil", Span::default()));
    c.start_if(Span::default());
    c.add(Instruction::EmitRaw("nope1", Span::default()));
    c.start_else(Span::default());
    c.add(Instruction::EmitRaw("yes", Span::default()));
    c.end_if();
    c.end_if();

    insta::assert_debug_snapshot!(&c.finish());
}

#[test]
fn test_bool_ops() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);

    c.start_sc_bool();
    c.add(Instruction::Lookup("first", Span::default()));
    c.sc_bool(true, Span::default(), vec![]);
    c.add(Instruction::Lookup("second", Span::default()));
    c.sc_bool(false, Span::default(), vec![]);
    c.add(Instruction::Lookup("third", Span::default()));
    c.end_sc_bool();

    insta::assert_debug_snapshot!(&c.finish());
}

#[test]
fn test_const() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);

    c.add(Instruction::LoadConst(Value::from("a")));
    c.add(Instruction::LoadConst(Value::from(42)));
    c.add(Instruction::StringConcat(Span::default()));

    insta::assert_debug_snapshot!(&c.finish());
}

#[test]
fn test_referenced_names_empty_bug() {
    let c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    let instructions = c.finish().0;
    let rv = instructions.get_referenced_names(0);
    assert!(rv.is_empty());
}
