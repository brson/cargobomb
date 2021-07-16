#[cfg(feature = "docs_rs_feature")]
compile_error!("oh no, a hidden regression!");

fn main() {
    println!("Hello, world!");
}
