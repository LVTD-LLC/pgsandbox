use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match pgsandbox_mcp::cli::run(std::env::args().skip(1).collect()).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("pgsandbox-mcp failed to start: {error}");
            ExitCode::from(1)
        }
    }
}
