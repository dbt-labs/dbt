use std::path::PathBuf;

use minijinja::value::{Value, ViaDeserialize};
use minijinja::{context, Environment};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Point {
    x: f32,
    y: f32,
}

fn dirname(path: ViaDeserialize<PathBuf>) -> String {
    match path.parent() {
        Some(parent) => parent.display().to_string(),
        None => "".to_string(),
    }
}

fn point_as_tuple(point: ViaDeserialize<Point>) -> Value {
    Value::from(vec![point.x, point.y])
}

fn main() {
    let mut env = Environment::new();
    env.add_template("example.txt", include_str!("example.txt"))
        .unwrap();
    env.add_filter("dirname", dirname);
    env.add_filter("point_as_tuple", point_as_tuple);

    let point = Point { x: -1.0, y: 1.0 };

    // First example: shows ViaDeserialize
    let template = env.get_template("example.txt").unwrap();
    println!(
        "{}",
        template
            .render(context! {
                path => std::env::current_dir().unwrap(),
                point => point,
            })
            .unwrap()
    );

    // Second example shows how you can deserialize directly from a value
    let point_value = Value::from_serialize(&point);
    println!("Point serialized as value: {}", point_value);
    let point_again = Point::deserialize(point_value).unwrap();
    println!("Point deserialization: {:?}", point_again);
}
