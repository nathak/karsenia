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
                "-h" | "--help" => return Err("usage: smart-workbench-app [--port N] [--data FOLDER] [--claude PROGRAM]".into()),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(args)
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let args = match parse() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let config = swb_app::server::Config { port: args.port, data_dir: args.data.unwrap_or_else(swb_app::registry::data_dir), bin: args.claude };
    match swb_app::server::start(config).await {
        Ok(running) => {
            println!("SmartWorkbench App is running. Open:\n  {}", running.url());
            let _ = running.task.await;
        }
        Err(error) => {
            eprintln!("could not start: {error:#}");
            std::process::exit(1);
        }
    }
}
