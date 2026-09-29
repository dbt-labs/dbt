#![cfg(feature = "unstable_machinery")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::compiler::codegen::CodeGenerationProfile;
use minijinja::funcsign_parser;
use minijinja::load_builtins_with_namespace;
use minijinja::machinery::Span;
use minijinja::value::Value;
use minijinja::{
    Argument, DynTypeObject, Environment, TypecheckingEventListener, UserDefinedFunctionType,
};

/// Simple listener that collects warnings into a Vec.
/// Uses RefCell since TypecheckingEventListener is used via Rc (single-threaded).
struct WarningCollector {
    warnings: RefCell<Vec<String>>,
}

impl WarningCollector {
    fn new() -> Self {
        Self {
            warnings: RefCell::new(Vec::new()),
        }
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }
}

impl TypecheckingEventListener for WarningCollector {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn warn(&self, message: &str) {
        self.warnings.borrow_mut().push(message.to_string());
    }

    fn set_span(&self, _span: &Span) {}
    fn new_block(&self, _block_id: usize) {}
    fn flush(&self) {}
    fn on_lookup(&self, _span: &Span, _simple_name: &str, _full_name: &str, _def_spans: Vec<Span>) {
    }
}

/// Build the minimal typecheck context required by the type checker.
/// The type checker expects TARGET_PACKAGE_NAME, ROOT_PACKAGE_NAME,
/// DBT_AND_ADAPTERS_NAMESPACE and `dialect` to be present.
fn minimal_typecheck_context() -> BTreeMap<String, Value> {
    let mut ctx = BTreeMap::new();
    ctx.insert("TARGET_PACKAGE_NAME".to_string(), Value::from("test"));
    ctx.insert("ROOT_PACKAGE_NAME".to_string(), Value::from("test"));
    // DBT_AND_ADAPTERS_NAMESPACE is `dialect -> (macro_name -> package)`; both
    // levels are objects (ValueMap = IndexMap<Value, Value>).
    let inner = indexmap::IndexMap::<Value, Value>::new();
    let mut namespaces = indexmap::IndexMap::<Value, Value>::new();
    namespaces.insert(Value::from("postgres"), Value::from_object(inner));
    ctx.insert(
        "DBT_AND_ADAPTERS_NAMESPACE".to_string(),
        Value::from_object(namespaces),
    );
    ctx.insert("dialect".to_string(), Value::from("postgres"));
    ctx
}

/// Helper to run typecheck on a template source with a given function registry.
fn typecheck_template(source: &str, funcsigns: BTreeMap<String, DynTypeObject>) -> Vec<String> {
    let context = minimal_typecheck_context();
    let funcsigns = Arc::new(funcsigns);
    let builtins = load_builtins_with_namespace(None).unwrap();

    let mut env = Environment::new();
    env.profile = CodeGenerationProfile::TypeCheck(funcsigns.clone(), context.clone());

    let tmpl = env
        .template_from_named_str("test_template", source)
        .unwrap();

    let listener = Rc::new(WarningCollector::new());
    tmpl.typecheck(funcsigns, builtins, listener.clone(), context)
        .unwrap();

    listener.warnings()
}

/// Build a UserDefinedFunctionType from a funcsign string and parameter names.
fn build_udf_from_funcsign(name: &str, funcsign: &str, param_names: &[&str]) -> DynTypeObject {
    let builtins = load_builtins_with_namespace(None).unwrap();
    let (arg_types, ret_type) = funcsign_parser::parse(funcsign, builtins).unwrap();

    let args: Vec<Argument> = param_names
        .iter()
        .zip(arg_types.iter())
        .map(|(pname, ty)| Argument {
            name: (*pname).to_string(),
            type_: ty.clone(),
            is_optional: false,
        })
        .collect();

    DynTypeObject::new(Arc::new(UserDefinedFunctionType::new(
        name,
        args,
        ret_type,
        &PathBuf::from("test.sql"),
        &Span::default(),
        &format!("test.{name}"),
    )))
}

/// Register a UDF in the function registry under the qualified name
/// ("test.{name}"), which is what CodeGenerationProfile::TypeCheck uses
/// during compilation to resolve funcsign parameters.
fn register_udf(
    funcsigns: &mut BTreeMap<String, DynTypeObject>,
    name: &str,
    funcsign: &str,
    param_names: &[&str],
) {
    let udf = build_udf_from_funcsign(name, funcsign, param_names);
    funcsigns.insert(format!("test.{name}"), udf);
}

/// Register a UDF under both the qualified and bare name.
/// The CFG's current_macro uses bare names (from MacroName instruction),
/// so the per-block return type check (typemeta.rs:550) and last-block
/// implicit return check (typemeta.rs:592) require the bare name key.
fn register_udf_with_bare_name(
    funcsigns: &mut BTreeMap<String, DynTypeObject>,
    name: &str,
    funcsign: &str,
    param_names: &[&str],
) {
    let udf = build_udf_from_funcsign(name, funcsign, param_names);
    funcsigns.insert(format!("test.{name}"), udf.clone());
    funcsigns.insert(name.to_string(), udf);
}

/// Parse macro name, parameter names, and funcsign from a SQL template.
///
/// Expects the template to contain:
///   `{% macro NAME(param1, param2, ...) %}`
///   `{#-- funcsign: SIGNATURE --#}`
fn parse_macro_metadata(source: &str) -> (&str, Vec<&str>, &str) {
    let macro_start = source
        .find("macro ")
        .expect("SQL file must contain a macro definition");
    let after_macro = &source[macro_start + 6..];
    let paren_start = after_macro.find('(').expect("macro must have parameters");
    let name = after_macro[..paren_start].trim();
    let paren_end = after_macro
        .find(')')
        .expect("macro must have closing paren");
    let params_str = &after_macro[paren_start + 1..paren_end];
    let params: Vec<&str> = params_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let funcsign_marker = "funcsign:";
    let fs_start = source
        .find(funcsign_marker)
        .expect("SQL file must contain a funcsign comment");
    let after_fs = &source[fs_start + funcsign_marker.len()..];
    let fs_end = after_fs
        .find("--#")
        .expect("funcsign comment must end with --#");
    let funcsign = after_fs[..fs_end].trim();

    (name, params, funcsign)
}

fn typecheck_case(
    source: &str,
    register_fn: fn(&mut BTreeMap<String, DynTypeObject>, &str, &str, &[&str]),
) -> Vec<String> {
    let (name, params, funcsign) = parse_macro_metadata(source);
    let mut funcsigns_map = BTreeMap::new();
    register_fn(&mut funcsigns_map, name, funcsign, &params);
    typecheck_template(source, funcsigns_map)
}

#[test]
fn typecheck_udf_arithmetic_type_mismatch() {
    let source = r#"{%- macro my_macro(a, b) -%}
{#-- funcsign: (string, integer) -> string --#}
{{ a + b }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Type mismatch for +: lhs = String, rhs = Integer"
"###);
}

#[test]
fn typecheck_udf_both_branch_variable_definition() {
    let source = r#"{%- macro my_macro(cond) -%}
{#-- funcsign: (boolean) -> string --#}
{% if cond %}{% set x = 1 %}{% else %}{% set x = 2 %}{% endif %}
{{ x }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_comparison_type_mismatch() {
    let source = r#"{%- macro my_macro(a, b) -%}
{#-- funcsign: (string, integer) -> string --#}
{% if a > b %}yes{% endif %}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Type mismatch for >: lhs = String, rhs = Integer"
"###);
}

#[test]
fn typecheck_udf_for_loop_element_type_inference() {
    let source = r#"{%- macro my_macro(items) -%}
{#-- funcsign: (list[integer]) -> string --#}
{% for item in items %}{{ item + 1 }}{% endfor %}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_funcsign_is_not_none_narrowing() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (optional[integer]) -> string --#}
{% if a is not none %}
  {{ a + 1 }}
{% else %}
  {{ a }}
{% endif %}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_invalid_attribute_on_type() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> string --#}
{{ a.nonexistent() }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- Integer.nonexistent is not supported
"###);
}

#[test]
fn typecheck_udf_is_integer_else_narrowing() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (string | integer) -> string --#}
{% if a is integer %}{{ a + 1 }}{% else %}{{ a.upper() }}{% endif %}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_is_string_narrowing() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (string | integer) -> string --#}
{% if a is string %}{{ a.upper() }}{% endif %}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_nested_if_all_branches_define_var() {
    let source = r#"{%- macro my_macro(a, b) -%}
{#-- funcsign: (boolean, boolean) -> string --#}
{% if a %}
  {% if b %}{% set x = 1 %}{% else %}{% set x = 2 %}{% endif %}
{% else %}
  {% set x = 3 %}
{% endif %}
{{ x }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_nested_if_partial_branch_definition() {
    let source = r#"{%- macro my_macro(a, b) -%}
{#-- funcsign: (boolean, boolean) -> string --#}
{% if a %}
  {% if b %}{% set x = 1 %}{% else %}nope{% endif %}
{% else %}
  {% set x = 3 %}
{% endif %}
{{ x }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_nested_loop_variable() {
    let source = r#"{%- macro my_macro(matrix) -%}
{#-- funcsign: (list[list[integer]]) -> string --#}
{% for row in matrix %}{% for cell in row %}{{ cell + 1 }}{% endfor %}{% endfor %}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_non_integer_slice_index() {
    let source = r#"{%- macro my_macro(items) -%}
{#-- funcsign: (list[integer]) -> string --#}
{{ items["a":"b"] }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Type mismatch for slice b: type = List(Integer)"
- "Type mismatch for slice stop: type = String(b)"
"###);
}

#[test]
fn typecheck_udf_optional_attribute_access_no_guard() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (optional[string]) -> string --#}
{{ a.upper() }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "[String| None].upper is not supported"
"###);
}

#[test]
fn typecheck_udf_optional_attribute_access_with_guard() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (optional[string]) -> string --#}
{% if a is not none %}{{ a.upper() }}{% endif %}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_single_branch_variable_definition() {
    let source = r#"{%- macro my_macro(cond) -%}
{#-- funcsign: (boolean) -> string --#}
{% if cond %}{% set x = 1 %}{% else %}nope{% endif %}
{{ x }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Variable 'x' is not defined in one of its predecessor blocks."
"###);
}

#[test]
fn typecheck_udf_undefined_filter() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> string --#}
{{ a | not_a_filter }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Potential TypeError: Filter 'not_a_filter' is not defined."
"###);
}

#[test]
fn typecheck_udf_undefined_function() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> string --#}
{{ not_a_function() }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Potential TypeError: Function 'not_a_function' is not defined."
"###);
}

#[test]
fn typecheck_udf_undefined_macro_call() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> string --#}
{{ unknown_macro() }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Potential TypeError: Function 'unknown_macro' is not defined."
"###);
}

#[test]
fn typecheck_udf_unknown_local_variable() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> string --#}
{{ unknown_var }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Potential TypeError: Unknown local variable 'unknown_var'"
"###);
}

#[test]
fn typecheck_udf_unpack_list_length_mismatch() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> string --#}
{% set x, y = [1, 2, 3] %}
{{ x }}{{ y }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_valid_integer_slice_index() {
    let source = r#"{%- macro my_macro(items) -%}
{#-- funcsign: (list[integer]) -> string --#}
{{ items[0:2] }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Type mismatch for slice b: type = List(Integer)"
"###);
}

#[test]
fn typecheck_udf_valid_string_method() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (string) -> string --#}
{{ a.upper() }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_var_defined_in_loop_used_after() {
    let source = r#"{%- macro my_macro(items) -%}
{#-- funcsign: (list[integer]) -> string --#}
{% for item in items %}{% set x = item %}{% endfor %}
{{ x }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Potential TypeError: Unknown local variable 'x'"
"###);
}

#[test]
fn typecheck_udf_and_bare_name_correct_return_type() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> string --#}
{{ return("hello") }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf_with_bare_name);
    insta::assert_yaml_snapshot!(warnings, @r###"
[]
"###);
}

#[test]
fn typecheck_udf_and_bare_name_explicit_return_type_mismatch() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> integer --#}
{{ return("hello") }}
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf_with_bare_name);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Type mismatch: expected return type Integer, got String(hello)"
- "Type mismatch: expected return type Integer, got String(hello)"
- "Type mismatch: expected return type Integer, got String"
"###);
}

#[test]
fn typecheck_udf_and_bare_name_implicit_return_type_mismatch() {
    let source = r#"{%- macro my_macro(a) -%}
{#-- funcsign: (integer) -> integer --#}
hello world
{%- endmacro -%}"#;
    let warnings = typecheck_case(source, register_udf_with_bare_name);
    insta::assert_yaml_snapshot!(warnings, @r###"
- "Type mismatch: expected return type Integer, got None"
- "Type mismatch: expected return type Integer, got String"
"###);
}
