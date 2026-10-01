#![cfg(feature = "unstable_machinery")]
use minijinja::compiler::codegen::CodeGenerationProfile;
use minijinja::constants::{CURRENT_PATH, CURRENT_SPAN};
use minijinja::machinery::{CodeGenerator, Instruction, Instructions, Span, Vm};
use minijinja::value::Value;
use minijinja::{AutoEscape, Environment, Error};
use std::collections::BTreeMap;

use similar_asserts::assert_eq;

pub fn simple_eval<S: serde::Serialize>(
    instructions: &Instructions<'_>,
    ctx: S,
) -> Result<String, Error> {
    let env = Environment::new();
    let empty_blocks = BTreeMap::new();
    let vm = Vm::new(&env);
    let root = Value::from_serialize(&ctx);
    Ok(vm
        .eval(instructions, root, &empty_blocks, AutoEscape::None, &[])?
        .0
        .as_str()
        .unwrap()
        .to_string())
}

#[test]
fn test_loop() {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("items", Value::from((1..=9).collect::<Vec<_>>()));

    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::Lookup("items", Span::default()));
    c.start_for_loop(false, false, Span::default());
    c.add(Instruction::Emit(Span::default()));
    c.end_for_loop(false, Span::default());
    c.add(Instruction::EmitRaw("!", Span::default()));

    let output = simple_eval(&c.finish().0, ctx).unwrap();

    assert_eq!(output, "123456789!");
}

#[test]
fn test_if() {
    for &(val, expectation) in [(true, "true"), (false, "false")].iter() {
        let mut ctx = std::collections::BTreeMap::new();
        ctx.insert("cond", Value::from(val));

        let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
        c.add(Instruction::Lookup("cond", Span::default()));
        c.start_if(Span::default());
        c.add(Instruction::EmitRaw("true", Span::default()));
        c.start_else(Span::default());
        c.add(Instruction::EmitRaw("false", Span::default()));
        c.end_if();

        let output = simple_eval(&c.finish().0, ctx).unwrap();

        assert_eq!(output, expectation);
    }
}

#[test]
fn test_if_branches() {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("false", Value::from(false));
    ctx.insert("nil", Value::from(()));

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

    let output = simple_eval(&c.finish().0, ctx).unwrap();

    assert_eq!(output, "yes");
}

#[test]
fn test_basic() {
    let mut user = std::collections::BTreeMap::new();
    user.insert("name", "Peter");
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("user", Value::from(user));
    ctx.insert("a", Value::from(42));
    ctx.insert("b", Value::from(23));

    let mut i = Instructions::new("", "", None);
    i.add(Instruction::EmitRaw("Hello ", Span::default()));
    i.add(Instruction::Lookup("user", Span::default()));
    i.add(Instruction::GetAttr("name", Span::default()));
    i.add(Instruction::Emit(Span::default()));
    i.add(Instruction::Lookup("a", Span::default()));
    i.add(Instruction::Lookup("b", Span::default()));
    i.add(Instruction::Add(Span::default()));
    i.add(Instruction::Neg(Span::default()));
    i.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&i, ctx).unwrap();
    assert_eq!(output, "Hello Peter-65");
}

#[test]
fn test_error_info() {
    let mut c = CodeGenerator::new("hello.html", "", CodeGenerationProfile::Render);
    c.add(Instruction::EmitRaw("<h1>Hello</h1>\n", Span::default()));
    c.add(Instruction::Lookup("a_string", Span::default()));
    c.add(Instruction::Lookup("an_int", Span::default()));
    c.add(Instruction::Add(Span {
        start_line: 2,
        start_col: 1,
        start_offset: 1,
        end_line: 2,
        end_col: 10,
        end_offset: 11,
    }));

    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("a_string", Value::from("foo"));
    ctx.insert("an_int", Value::from(42));
    ctx.insert(CURRENT_PATH, Value::from("hello.html"));
    ctx.insert(CURRENT_SPAN, Value::from_serialize(Span::default()));

    let err = simple_eval(&c.finish().0, ctx).unwrap_err();
    assert_eq!(err.name(), Some("hello.html"));
    assert_eq!(
        err.span(),
        Some(Span {
            start_line: 2,
            start_col: 1,
            start_offset: 1,
            end_line: 2,
            end_col: 10,
            end_offset: 11,
        }),
    );
}

#[test]
fn test_op_eq() {
    let mut c = CodeGenerator::new("hello.html", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::Eq(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "True");

    let mut c = CodeGenerator::new("hello.html", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from(2)));
    c.add(Instruction::Eq(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "False");
}

#[test]
fn test_op_ne() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from("foo")));
    c.add(Instruction::Ne(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "True");

    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from("foo")));
    c.add(Instruction::LoadConst(Value::from("foo")));
    c.add(Instruction::Ne(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "False");
}

#[test]
fn test_op_lt() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from(2)));
    c.add(Instruction::Lt(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "True");

    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(2)));
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::Lt(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "False");
}

#[test]
fn test_op_gt() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from(2)));
    c.add(Instruction::Gt(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "False");

    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(2)));
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::Gt(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "True");
}

#[test]
fn test_op_lte() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::Lte(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "True");

    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(2)));
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::Lte(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "False");
}

#[test]
fn test_op_gte() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from(2)));
    c.add(Instruction::Gte(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "False");

    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::LoadConst(Value::from(1)));
    c.add(Instruction::Gte(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "True");
}

#[test]
fn test_op_not() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(0)));
    c.add(Instruction::Not(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "True");

    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(true)));
    c.add(Instruction::Not(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "False");
}

#[test]
fn test_string_concat() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from("foo")));
    c.add(Instruction::LoadConst(Value::from(42)));
    c.add(Instruction::StringConcat(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "foo42");
}

#[test]
fn test_unpacking() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from(vec!["bar", "foo"])));
    c.add(Instruction::UnpackList(2, Span::default()));
    c.add(Instruction::StringConcat(Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "foobar");
}

#[test]
fn test_call_object() {
    let mut c = CodeGenerator::new("<unknown>", "", CodeGenerationProfile::Render);
    c.add(Instruction::LoadConst(Value::from_function(|a: u64| {
        42 + a
    })));
    c.add(Instruction::LoadConst(Value::from(23i32)));
    c.add(Instruction::CallObject(Some(2), Span::default()));
    c.add(Instruction::Emit(Span::default()));

    let output = simple_eval(&c.finish().0, ()).unwrap();
    assert_eq!(output, "65");
}

#[test]
#[cfg(feature = "stacker")]
fn test_deep_recursion() {
    use minijinja::context;

    let mut env = Environment::new();
    let limit = if cfg!(target_arch = "wasm32") {
        500
    } else {
        10000
    };
    env.set_recursion_limit(limit * 7);
    let tmpl = env
        .template_from_str(
            r#"
        {%- macro foo(i) -%}
            {%- if i < limit %}{{ i }}|{{ foo(i + 1) }}{%- endif -%}
        {%- endmacro -%}
        {{- foo(0) -}}
    "#,
        )
        .unwrap();
    let rv = tmpl.render(context!(limit), &[]).unwrap();
    let pieces = rv
        .split('|')
        .filter_map(|x| x.parse::<usize>().ok())
        .collect::<Vec<_>>();
    assert_eq!(pieces, (0..limit).collect::<Vec<_>>());
}
