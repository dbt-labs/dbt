use std::collections::BTreeMap;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde::Serialize;

use handlebars::handlebars_helper;

criterion_main! { benches }

criterion_group! {
    benches,
    bench_compare_compile,
    bench_compare_render,
}

#[derive(Serialize, Debug)]
struct NavItem {
    url: &'static str,
    title: &'static str,
    is_active: bool,
}

#[derive(Serialize, Debug)]
struct Site {
    nav: Vec<NavItem>,
    copyright: u32,
}

#[derive(Serialize, Debug, askama::Template)]
#[template(path = "comparison/askama.html")]
struct Context {
    items: Vec<String>,
    site: Site,
    title: &'static str,
}

#[derive(Serialize, Debug, rinja::Template)]
#[template(path = "comparison/askama.html")]
struct RinjaContext {
    items: Vec<String>,
    site: Site,
    title: &'static str,
}

macro_rules! default_context {
    ($($ty:ident),+) => {
        $(impl Default for $ty {
            fn default() -> Self {
                Self {
                    items: vec![
                        "<First Item>".into(),
                        "<Second Item>".into(),
                        "<Third Item>".into(),
                        "<Fourth Item>".into(),
                        "<Fifth Item>".into(),
                        "<Sixth Item>".into(),
                    ],
                    site: Site {
                        nav: vec![
                            NavItem {
                                url: "/",
                                title: "Index",
                                is_active: true,
                            },
                            NavItem {
                                url: "/download",
                                title: "Download",
                                is_active: false,
                            },
                            NavItem {
                                url: "/about",
                                title: "About",
                                is_active: false,
                            },
                            NavItem {
                                url: "/help",
                                title: "Help",
                                is_active: false,
                            },
                        ],
                        copyright: 2022,
                    },
                    title: "My Benchmark Site",
                }
            }
        })+
    }
}

default_context!(Context, RinjaContext);

pub fn bench_compare_compile(c: &mut Criterion) {
    let mut g = c.benchmark_group("cmp_compile");

    g.bench_function("minijinja", |b| {
        let index_source = include_str!("../inputs/comparison/minijinja.html");
        let footer_source = include_str!("../inputs/comparison/minijinja_footer.html");
        let mut env = minijinja::Environment::new();
        b.iter(|| {
            env.add_template("template.html", black_box(index_source))
                .unwrap();
            env.add_template("footer.html", black_box(footer_source))
                .unwrap();
        });
    });

    g.bench_function("tera", |b| {
        let index_source = include_str!("../inputs/comparison/tera.html");
        let footer_source = include_str!("../inputs/comparison/tera_footer.html");
        let mut tera = tera::Tera::default();
        b.iter(|| {
            tera.add_raw_template("template.html", black_box(index_source))
                .unwrap();
            tera.add_raw_template("footer.html", black_box(footer_source))
                .unwrap();
        });
    });

    g.bench_function("liquid", |b| {
        let index_source = include_str!("../inputs/comparison/liquid.html");
        let footer_source = include_str!("../inputs/comparison/liquid_footer.html");
        let parser = liquid::ParserBuilder::with_stdlib().build().unwrap();
        let mut templates = BTreeMap::new();
        b.iter(|| {
            templates.insert(
                "template.html",
                parser.parse(black_box(index_source)).unwrap(),
            );
            templates.insert(
                "footer.html",
                parser.parse(black_box(footer_source)).unwrap(),
            );
        });
    });

    g.bench_function("handlebars", |b| {
        let index_source = include_str!("../inputs/comparison/handlebars.html");
        let footer_source = include_str!("../inputs/comparison/handlebars_footer.html");
        let mut hbs = handlebars::Handlebars::new();
        b.iter(|| {
            hbs.register_template_string("template.html", black_box(index_source))
                .unwrap();
            hbs.register_template_string("footer.html", black_box(footer_source))
                .unwrap();
        });
    });
}

pub fn bench_compare_render(c: &mut Criterion) {
    let mut g = c.benchmark_group("cmp_render");

    g.bench_function("minijinja", |b| {
        let index_source = include_str!("../inputs/comparison/minijinja.html");
        let footer_source = include_str!("../inputs/comparison/minijinja_footer.html");
        let mut env = minijinja::Environment::new();
        env.add_template("template.html", index_source).unwrap();
        env.add_template("footer.html", footer_source).unwrap();
        b.iter(|| {
            env.get_template("template.html")
                .unwrap()
                .render(black_box(Context::default()))
                .unwrap();
        });
    });

    g.bench_function("tera", |b| {
        let index_source = include_str!("../inputs/comparison/tera.html");
        let footer_source = include_str!("../inputs/comparison/tera_footer.html");
        let mut tera = tera::Tera::default();
        tera.add_raw_template("template.html", index_source)
            .unwrap();
        tera.add_raw_template("footer.html", footer_source).unwrap();
        b.iter(|| {
            let ctx = black_box(tera::Context::from_serialize(Context::default()).unwrap());
            tera.render("template.html", &ctx).unwrap();
        });
    });

    g.bench_function("liquid", |b| {
        pub type Partials = liquid::partials::EagerCompiler<liquid::partials::InMemorySource>;
        let index_source = include_str!("../inputs/comparison/liquid.html");
        let footer_source = include_str!("../inputs/comparison/liquid_footer.html");
        let mut partials = Partials::empty();
        partials.add("footer.html", footer_source);
        let parser = liquid::ParserBuilder::with_stdlib()
            .partials(partials)
            .build()
            .unwrap();
        let mut templates = BTreeMap::new();
        templates.insert("template.html", parser.parse(index_source).unwrap());
        b.iter(|| {
            templates
                .get("template.html")
                .unwrap()
                .render(&black_box(liquid::to_object(&Context::default())).unwrap())
                .unwrap();
        });
    });

    g.bench_function("handlebars", |b| {
        handlebars_helper!(upper: |s: String| s.to_uppercase());
        let index_source = include_str!("../inputs/comparison/handlebars.html");
        let footer_source = include_str!("../inputs/comparison/handlebars_footer.html");
        let mut hbs = handlebars::Handlebars::new();
        hbs.register_template_string("template.html", index_source)
            .unwrap();
        hbs.register_template_string("footer.html", footer_source)
            .unwrap();
        hbs.register_helper("upper", Box::new(upper));
        b.iter(|| {
            hbs.render("template.html", &black_box(Context::default()))
                .unwrap();
        });
    });

    g.bench_function("rinja", |b| {
        b.iter(|| {
            let context = black_box(RinjaContext::default());
            rinja::Template::render(&context).unwrap();
        });
    });

    g.bench_function("askama", |b| {
        b.iter(|| {
            let context = black_box(Context::default());
            askama::Template::render(&context).unwrap();
        });
    });
}
