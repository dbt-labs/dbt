// include the generated file
include!(concat!(env!("OUT_DIR"), "/example.rs"));

fn main() {
    println!("build cwd: {BUILD_CWD}");
    for point in POINTS {
        println!("({}, {})", point.x, point.y);
    }
}
