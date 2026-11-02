fn main() {
    if let Err(error) = repvix::cli::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
