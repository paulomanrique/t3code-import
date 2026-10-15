fn main() {
    #[cfg(feature = "gui")]
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        tauri_build::build();
    }
}
