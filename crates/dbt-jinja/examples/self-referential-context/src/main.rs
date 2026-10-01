use std::sync::Arc;

use minijinja::value::{Enumerator, Object, Value, ValueKind};
use minijinja::{context, Environment};

#[derive(Debug)]
struct SelfReferentialContext {
    ctx: Value,
}

impl Object for SelfReferentialContext {
    fn get_value(self: &Arc<Self>, name: &Value) -> Option<Value> {
        if name.as_str() == Some("CONTEXT") {
            return Some(self.ctx.clone());
        }
        self.ctx.get_item(name).ok().filter(|x| !x.is_undefined())
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        if self.ctx.kind() == ValueKind::Map {
            if let Ok(keys) = self.ctx.try_iter() {
                return Enumerator::Values(keys.collect());
            }
        }
        Enumerator::Seq(0)
    }
}

pub fn make_self_referential(ctx: Value) -> Value {
    Value::from_object(SelfReferentialContext { ctx })
}

static TEMPLATE: &str = r#"
name: {{ name }}
CONTEXT.name: {{ CONTEXT.name }}
CONTEXT.CONTEXT is undefined: {{ CONTEXT.CONTEXT is undefined }}
CONTEXT: {{ CONTEXT }}
"#;

fn main() {
    let env = Environment::new();
    let template = env.template_from_str(TEMPLATE).unwrap();

    let ctx = make_self_referential(context! {
        name => "John",
        other_value => 42,
    });

    println!("{}", template.render(ctx).unwrap());
}
