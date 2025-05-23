//! Pool CLI entry point.
//!
//! This binary parses CLI arguments, loads the TOML configuration,
//! and starts the main runtime via `lib::start`.
//!
//! Task orchestration and shutdown are handled in `lib/mod.rs`.

#![allow(special_module_name)]

mod lib;
use ext_config::{Config, File, FileFormat};
pub use lib::{config, status, PoolSv2};
use tokio::select;
use tracing::{error, info};

mod args {
    use std::path::PathBuf;

    /// Args representing the config path.
    #[derive(Debug)]
    pub struct Args {
        pub config_path: PathBuf,
    }

    /// Internal state machine for CLI argument parsing.
    enum ArgsState {
        Next,
        ExpectPath,
        Done,
    }

    /// Parsing result for CLI arguments.
    enum ArgsResult {
        Config(PathBuf),
        None,
        Help(String),
    }

    impl Args {
        const DEFAULT_CONFIG_PATH: &'static str = "pool-config.toml";
        const HELP_MSG: &'static str =
            "Usage: -h/--help, -c/--config <path|default pool-config.toml>";
        /// Parses CLI arguments and returns the selected configuration path.
        ///
        /// Displays help message if requested or defaults to a predefined path.
        pub fn from_args() -> Result<Self, String> {
            let cli_args = std::env::args();

            if cli_args.len() == 1 {
                println!("Using default config path: {}", Self::DEFAULT_CONFIG_PATH);
                println!("{}\n", Self::HELP_MSG);
            }

            let config_path = cli_args
                .scan(ArgsState::Next, |state, item| {
                    match std::mem::replace(state, ArgsState::Done) {
                        ArgsState::Next => match item.as_str() {
                            "-c" | "--config" => {
                                *state = ArgsState::ExpectPath;
                                Some(ArgsResult::None)
                            }
                            "-h" | "--help" => Some(ArgsResult::Help(Self::HELP_MSG.to_string())),
                            _ => {
                                *state = ArgsState::Next;

                                Some(ArgsResult::None)
                            }
                        },
                        ArgsState::ExpectPath => Some(ArgsResult::Config(PathBuf::from(item))),
                        ArgsState::Done => None,
                    }
                })
                .last();
            let config_path = match config_path {
                Some(ArgsResult::Config(p)) => p,
                Some(ArgsResult::Help(h)) => return Err(h),
                _ => PathBuf::from(Self::DEFAULT_CONFIG_PATH),
            };
            Ok(Self { config_path })
        }
    }
}

/// Initializes logging, parses arguments, loads configuration, and starts the Pool runtime.
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = match args::Args::from_args() {
        Ok(cfg) => cfg,
        Err(help) => {
            error!("{}", help);
            return;
        }
    };

    let config_path = args.config_path.to_str().expect("Invalid config path");

    // Load config
    let config: config::PoolConfig = match Config::builder()
        .add_source(File::new(config_path, FileFormat::Toml))
        .build()
    {
        Ok(settings) => match settings.try_deserialize::<config::PoolConfig>() {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to deserialize config: {}", e);
                return;
            }
        },
        Err(e) => {
            error!("Failed to build config: {}", e);
            return;
        }
    };
    let _ = PoolSv2::new(config).start().await;
    select! {
        interrupt_signal = tokio::signal::ctrl_c() => {
            match interrupt_signal {
                Ok(()) => {
                    info!("Pool(bin): Caught interrupt signal. Shutting down...");
                    return;
                },
                Err(err) => {
                    error!("Pool(bin): Unable to listen for interrupt signal: {}", err);
                    return;
                },
            }
        }
    };
}
