mod model;
mod rdp;
mod store;

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod credentials;
#[cfg(target_os = "macos")]
mod login;

fn main() {
    #[cfg(target_os = "macos")]
    app::run();

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("tinyRDP requires macOS 13 or newer.");
        std::process::exit(1);
    }
}
