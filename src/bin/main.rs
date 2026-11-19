use clap_free::parse;

mod clap_free {
    pub struct Args {
        pub port: u16,
        pub data: Option<std::path::PathBuf>,
        pub claude: Option<String>,
        pub terminal: bool,
        /// The live view's port (`--live` for the default, `--live-port N`), and whether prompts are cut short in it.
        pub live: Option<u16>,
        pub live_short: bool,
    }

    pub fn parse() -> Result<Args, String> {
        let mut args = Args { port: 0, data: None, claude: None, terminal: false, live: None, live_short: false };
        let mut it = std::env::args().skip(1);
        while let Some(flag) = it.next() {
            match flag.as_str() {
                "--port" => args.port = it.next().and_then(|v| v.parse().ok()).ok_or("--port needs a number")?,
                "--data" => args.data = Some(it.next().ok_or("--data needs a folder")?.into()),
                "--claude" => args.claude = Some(it.next().ok_or("--claude needs a program")?),
                "--terminal" => args.terminal = true,
                "--live" => args.live = Some(tenrec::live::server::DEFAULT_PORT),
                "--live-port" => args.live = Some(it.next().and_then(|v| v.parse().ok()).ok_or("--live-port needs a number")?),
                "--live-short" => args.live_short = true,
                "-h" | "--help" => return Err("usage: tenrec [--port N] [--data FOLDER] [--claude PROGRAM] [--terminal] [--live | --live-port N] [--live-short]".into()),
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
    // `tenrec guard --data <folder> --sid <chat>`: the PreToolUse hook of a chat's claude (the app's own guard), not the app itself.
    if std::env::args_os().nth(1).is_some_and(|a| a == "guard") {
        let rest: Vec<String> = std::env::args_os().skip(2).map(|a| a.to_string_lossy().into_owned()).collect();
        std::process::exit(tenrec::workbench::hook::run(&rest));
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
    let terminal = args.terminal || std::env::var("TENREC_TERMINAL").map(|v| v == "1").unwrap_or(false);
    #[cfg(windows)]
    {
        if terminal {
            eprintln!("the terminal is not available on Windows");
            std::process::exit(2);
        }
        // No mode bits are set on Windows: the data folder relies on the default access list of the user's profile.
        let inside = tenrec::platform::home_dir().is_some_and(|profile| tenrec::platform::data_dir_in_profile(&config.data_dir, &profile));
        if !inside {
            eprintln!("the data folder must be inside your user profile folder on Windows (use --data with a folder there)");
            std::process::exit(2);
        }
    }
    match tenrec::server::start_with(config, terminal).await {
        Ok(running) => {
            println!("Tenrec is running. Open:\n  {}", running.url());
            let tenrec::server::Running { registry, task, .. } = running;
            // The live view (read only, its own port and token): kept for as long as the app runs.
            let _live = match args.live {
                Some(port) => match tenrec::live::server::start(registry.clone(), port, if args.live_short { tenrec::live::Prompts::Short } else { tenrec::live::Prompts::Full }).await {
                    Ok(live) => {
                        println!("Live view (read only; it shows prompts and commands, secrets masked):\n  {}", live.url(None, "ko"));
                        Some(live)
                    }
                    Err(error) => {
                        eprintln!("the live view could not start: {error:#}");
                        None
                    }
                },
                None => None,
            };
            tokio::select! {
                _ = task => {}
                _ = tokio::signal::ctrl_c() => {}
                _ = termination() => {}
            }
            // Leave no claude, and nothing it started, behind.
            registry.kill_all();
            tenrec::server::term_ops::kill_all();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        Err(error) => {
            eprintln!("could not start: {error:#}");
            std::process::exit(1);
        }
    }
}

/// Resolves when the system asks this program to end: SIGTERM, or on Windows the console being closed.
#[cfg(unix)]
async fn termination() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut terminate) => {
            terminate.recv().await;
        }
        Err(_) => std::future::pending().await,
    }
}

#[cfg(windows)]
async fn termination() {
    match tokio::signal::windows::ctrl_close() {
        Ok(mut close) => {
            close.recv().await;
        }
        Err(_) => std::future::pending().await,
    }
}
