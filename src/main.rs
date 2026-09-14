use any_a2a::service;
use std::time::Duration;
fn main() {
    if let Err(error) = run() {
        eprintln!("any-a2a: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--help" | "-h") | None => {
            println!(
                "any-a2a run (--card URL | --card-file PATH) --message TEXT [--timeout-secs 120]\nA2A JSON-RPC client. Optional bearer token: ANY_A2A_TOKEN.\nTimeout or local termination does not cancel remote work."
            );
            return Ok(());
        }
        Some("serve") => return service::serve(),
        Some("run") => {}
        _ => return Err("Expected run; see --help".into()),
    }
    let (mut card, mut card_file, mut message, mut timeout) = (None, None, None, 120u64);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        match flag.as_str() {
            "--card" => card = Some(value),
            "--card-file" => card_file = Some(value),
            "--message" => message = Some(value),
            "--timeout-secs" => timeout = value.parse().map_err(|_| "Invalid timeout")?,
            _ => return Err(format!("Unknown option {flag}")),
        }
    }
    if timeout == 0 || timeout > 86400 {
        return Err("Timeout must be 1..86400 seconds".into());
    }
    let message = message.ok_or("Missing --message")?;
    let token = std::env::var("ANY_A2A_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let duration = Duration::from_secs(timeout);
    let mut client = match (card, card_file) {
        (Some(url), None) => any_a2a::A2aClient::connect(&url, token, duration)?,
        (None, Some(path)) => {
            let file = std::fs::File::open(path).map_err(|_| "Cannot open local Agent Card")?;
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "Cannot read Agent Card")?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err("Agent Card too large".into());
            }
            let card =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid local Agent Card JSON")?;
            any_a2a::A2aClient::connect_card(&card, token, duration)?
        }
        _ => return Err("Specify exactly one of --card or --card-file".into()),
    };
    let result = client.run(&message)?;
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|_| "Result serialization failed")?
    );
    Ok(())
}
