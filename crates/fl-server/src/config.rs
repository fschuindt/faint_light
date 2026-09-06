use std::path::PathBuf;
use std::time::Duration;

/// Server configuration, from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub index_dir: PathBuf,
    pub cache_gb: f64,
    /// Optional rig pixel-scale prior, arcsec/px.
    pub scale_lo: Option<f64>,
    pub scale_hi: Option<f64>,
    pub solve_timeout: Duration,
    /// Fake solver for API integration testing (FAINT_LIGHT_FAKE=1).
    pub fake: bool,
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

impl Config {
    pub fn from_env() -> Config {
        let index_dir = std::env::var("FAINT_LIGHT_INDEX_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let docker_default = PathBuf::from("/index");
                if docker_default.is_dir() {
                    docker_default
                } else {
                    PathBuf::from("./indexes")
                }
            });
        Config {
            port: env_parse("FAINT_LIGHT_PORT").unwrap_or(8000),
            index_dir,
            cache_gb: env_parse("FAINT_LIGHT_CACHE_GB").unwrap_or(2.0),
            scale_lo: env_parse("FAINT_LIGHT_SCALE_LOW"),
            scale_hi: env_parse("FAINT_LIGHT_SCALE_HIGH"),
            solve_timeout: Duration::from_secs(
                env_parse("FAINT_LIGHT_SOLVE_TIMEOUT").unwrap_or(300),
            ),
            fake: std::env::var("FAINT_LIGHT_FAKE").is_ok_and(|v| v == "1" || v == "true"),
        }
    }
}
