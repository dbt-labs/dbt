use minijinja::render;

fn main() {
    println!("{}", render!("Hello {{ name }}!", name => "John"));
}
