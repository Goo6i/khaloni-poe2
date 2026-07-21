fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("");
    match mode {
        "--calibrate" => {
            eprintln!("calibration arrives in Task A4");
            Ok(())
        }
        "--headless" => {
            eprintln!("headless pipeline arrives in Task A4");
            Ok(())
        }
        _ => {
            eprintln!("overlay arrives in Stage B; use --headless or --calibrate for now");
            Ok(())
        }
    }
}
