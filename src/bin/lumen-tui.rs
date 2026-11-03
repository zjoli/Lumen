fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: no runtime or worker thread exists at process startup.
    unsafe {
        lumen::versioning::configure_transport_timeouts(15_000, 60_000)?;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(lumen::tui::run())
}
