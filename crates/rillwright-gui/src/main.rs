mod app;
mod conversation;
mod settings;
mod startup;

fn main() -> eframe::Result {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("rillwright-gui {}", env!("DORY_VERSION"));
        return Ok(());
    }
    let launch = startup::parse_args(args).map_err(|e| {
        eprintln!(
            "rillwright-gui: {e}\nusage: rillwright-gui [--demo] [--model PATH] [--workspace DIR]"
        );
        eframe::Error::AppCreation(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            e,
        )))
    })?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1050.0, 760.0])
            .with_min_inner_size([650.0, 480.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Rillwright",
        options,
        Box::new(move |cc| Ok(Box::new(app::RillwrightApp::new(cc, launch)))),
    )
}

fn app_icon() -> eframe::egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/rillwright.png"))
        .expect("embedded Rillwright app icon is a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();

    eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}
