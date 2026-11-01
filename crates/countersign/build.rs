//! Builds the mascot icons in `data/icons` into the binary, so the app
//! shows them before it is installed, in the demo and in screenshots.

fn main() {
    glib_build_tools::compile_resources(
        &["../../data/icons"],
        "../../data/countersign.gresource.xml",
        "countersign.gresource",
    );
}
