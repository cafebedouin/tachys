//! The `tachys` program: the library's one call, with this process's arguments, stdin and stdout.
fn main() {
    std::process::exit(tachys::cli())
}
