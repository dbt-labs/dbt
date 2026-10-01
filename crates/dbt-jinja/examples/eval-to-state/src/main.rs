use minijinja::{args, context, Environment};

fn main() {
    let mut env = Environment::new();
    env.add_template("layout.html", include_str!("templates/layout.html"))
        .unwrap();
    env.add_template("index.html", include_str!("templates/index.html"))
        .unwrap();

    let template = env.get_template("index.html").unwrap();
    let mut state = template
        .eval_to_state(context! {
            site_url => "http://example.com",
        })
        .unwrap();

    println!("Block 'title': {:?}", state.render_block("title").unwrap());
    println!("Block 'body': {:?}", state.render_block("body").unwrap());
    println!(
        "Macro 'utility': {:?}",
        state.call_macro("utility", args!()).unwrap()
    );
    println!(
        "Variable 'global_variable': {:?}",
        state.lookup("global_variable")
    );
    println!("Exports: {:?}", state.exports());

    println!("Template name: {:?}", state.name());
    println!("Undefined behavior: {:?}", state.undefined_behavior());
    println!(
        "Range function resolved: {:?}",
        state.lookup("range").unwrap()
    );
    println!(
        "Range function invoked: {:?}",
        state
            .lookup("range")
            .unwrap()
            .call(&state, args!(5))
            .unwrap()
    );
}
