fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/io.github.vextheater.gresource.xml",
        "compiled.gresource",
    );
}
