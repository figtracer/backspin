//! `backspin-render`: renders audio through the engine with the harness flags.

fn main() {
    let args = match backspin_render::Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = backspin_render::run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
