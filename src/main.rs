mod app;
mod bedrock;
mod db;
mod keychain;
mod math_render;
mod md_render;
mod message;

use eframe::egui;
use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    let rt_handle = rt.handle().clone();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Bedrock Chat")
            .with_inner_size([1100.0, 750.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Bedrock Chat",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::ChatApp::new(cc, rt_handle)))),
    )
}
