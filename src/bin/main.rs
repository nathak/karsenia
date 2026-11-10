use clap_free::parse;

mod clap_free {
    pub struct Args {
        pub port: u16,
        pub data: Option<std::path::PathBuf>,
        pub claude: Option<String>,
    }

    pub fn parse() -> Result<Args, String> {
        let mut args = Args { port: 0, data: None, claude: None };
        let mut it = std::env::args().skip(1);
        while let Some(flag) = it.next() {
            match flag.as_str() {
                "--port" => args.port = it.next().and_then(|v| v.parse().ok()).ok_or("--port needs a number")?,
                "--data" => args.data = Some(it.next().ok_or("--data needs a folder")?.into()),
                "--claude" => args.claude = Some(it.next().ok_or("--claude needs a program")?),
                "-h" | "--help" => return Err("usage: tenrec [--port N] [--data FOLDER] [--claude PROGRAM]".into()),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(args)
    }
}

#[tokio::main]
async fn main() {
    // `tenrec mcp`: the MCP server that claude starts for a chat (stdio), not the app itself.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        std::process::exit(tenrec::mcp::run_stdio().await);
    }
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let args = match parse() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let config = tenrec::server::Config { port: args.port, data_dir: args.data.unwrap_or_else(tenrec::registry::data_dir), bin: args.claude, mcp_bin: std::env::current_exe().ok(), extract: Default::default(), propose: Default::default(), sched: Default::default() };
    match tenrec::server::start(config).await {
        Ok(running) => {
            println!("Tenrec is running. Open:\n  {}", running.url());
            let tenrec::server::Running { registry, task, .. } = running;
            let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("could not listen for SIGTERM");
            tokio::select! {
                _ = task => {}
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            // Leave no claude, and nothing it started, behind.
            registry.kill_all();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        Err(error) => {
            eprintln!("could not start: {error:#}");
            std::process::exit(1);
        }
    }
}
