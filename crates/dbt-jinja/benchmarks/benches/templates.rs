use criterion::{black_box, criterion_group, criterion_main, Criterion};
use minijinja::machinery::parse;
use minijinja::{context, Environment, State};

fn do_parse() {
    parse(
        black_box(include_str!("../inputs/all_elements.html")),
        "all_elements.html",
        Default::default(),
        Default::default(),
    )
    .unwrap();
}

fn do_parse_and_compile() {
    let mut env = Environment::new();
    env.add_template(
        "all_elements.html",
        include_str!("../inputs/all_elements.html"),
    )
    .unwrap();
}

fn do_render(env: &Environment) {
    let tmpl = env.get_template("all_elements.html").unwrap();
    tmpl.render(context! {
        DEBUG => false,
        site => context! {
            nav => vec![
                context!{url => "/", is_active => true, title => "Index"},
                context!{url => "/doc", is_active => false, title => "Docs"},
                context!{url => "/help", is_active => false, title => "Help"},
            ]
        },
        items => (0..200).skip(3).collect::<Vec<_>>(),
    })
    .unwrap();
}

fn create_real_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.add_template("footer.html", include_str!("../inputs/footer.html"))
        .unwrap();
    env.add_template(
        "all_elements.html",
        include_str!("../inputs/all_elements.html"),
    )
    .unwrap();
    env.add_filter("asset_url", |_: &State, value: String| Ok(value));
    env.add_function("current_year", |_: &State| Ok(2022));
    env
}

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("parse", |b| b.iter(do_parse));
    c.bench_function("compile", |b| b.iter(do_parse_and_compile));
    c.bench_function("render", |b| {
        let env = create_real_env();
        b.iter(|| do_render(&env));
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
