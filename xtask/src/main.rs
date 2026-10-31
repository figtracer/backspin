mod parity;

// The plugin checks load the bundles and FL's libraries with dlopen.
#[cfg(target_os = "macos")]
mod plugin_check;
#[cfg(target_os = "macos")]
mod vst3_check;

fn main() -> nih_plug_xtask::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("parity") {
        if let Err(e) = parity::run(&args[1..]) {
            eprintln!("parity: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if args.first().map(String::as_str) == Some("plugin-check") {
        if let Err(e) = plugin_check::run(&args[1..]) {
            eprintln!("plugin-check: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("preset-pack") {
        let out = args.get(1).map_or("target/presets.pack", String::as_str);
        match backspin_presets::collect() {
            Ok(pack) => {
                let bytes = pack.encode();
                if let Err(e) = std::fs::write(out, &bytes) {
                    eprintln!("preset-pack: {out}: {e}");
                    std::process::exit(1);
                }
                println!(
                    "{out}: {} Gross Beat presets, {} maps, {} shape presets in {} groups, {} fonts, {} bytes",
                    pack.gross_beat.len(),
                    pack.maps.len(),
                    pack.shapes.len(),
                    pack.groups.len(),
                    pack.fonts.len(),
                    bytes.len()
                );
            }
            Err(e) => {
                eprintln!("preset-pack: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    nih_plug_xtask::main()
}
