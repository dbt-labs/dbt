use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to TOML config file
    #[arg(long, default_value = "adbc_conf.toml")]
    config: PathBuf,

    /// Profile name in config file
    #[arg(long, default_value = "default")]
    profile: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let result = dbt_adbc::repl::run_repl(&args.config, &args.profile).await;
    let code = if let Err(err) = result {
        std::eprintln!("{}", err);
        1
    } else {
        0
    };
    std::process::exit(code);
}
