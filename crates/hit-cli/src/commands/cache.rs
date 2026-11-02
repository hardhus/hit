use std::process::ExitCode;

use hit_cache::Cache;
use hit_config::Paths;

use crate::{accounts, cli::CacheCommand, style};

pub fn run(command: &CacheCommand, paths: &Paths) -> ExitCode {
    let path = accounts::cache_path(paths);
    match command {
        CacheCommand::Clear => {
            Cache::remove(&path);
            println!("{} cleared the local cache", style::ok("✓"));
        }
        CacheCommand::Path => println!("{}", path.display()),
    }
    ExitCode::SUCCESS
}
