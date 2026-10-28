use std::path::Path;

use hit_config::Paths;

pub fn run(paths: &Paths, config_file: &Path) {
    let rows = [
        ("config file", config_file),
        ("config", paths.config.as_path()),
        ("data", paths.data.as_path()),
        ("state", paths.state.as_path()),
        ("cache", paths.cache.as_path()),
        ("logs", paths.logs.as_path()),
    ];
    for (name, path) in rows {
        println!("{name:<12} {}", path.display());
    }
}
