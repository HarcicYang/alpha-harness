fn main() {
    if let Err(error) = alpha_build::check_and_generate() {
        eprintln!("composition error: {error}");
        std::process::exit(1);
    }
}
