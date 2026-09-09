use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Server configuration, from environment variables.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Interface to listen on. `0.0.0.0` reaches the whole LAN; `127.0.0.1`
    /// keeps the server to this machine.
    pub bind: IpAddr,
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
            bind: env_parse("FAINT_LIGHT_BIND").unwrap_or(IpAddr::from([0, 0, 0, 0])),
            port: env_parse("FAINT_LIGHT_PORT").unwrap_or(7222),
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

    /// True when `other` differs in a way the solve engine depends on, as
    /// opposed to just where it listens.
    pub fn engine_differs(&self, other: &Config) -> bool {
        self.index_dir != other.index_dir
            || self.cache_gb != other.cache_gb
            || self.scale_lo != other.scale_lo
            || self.scale_hi != other.scale_hi
            || self.solve_timeout != other.solve_timeout
            || self.fake != other.fake
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_engine_inputs_force_a_rebuild() {
        let a = Config::from_env();
        let mut b = a.clone();
        b.port = a.port + 1;
        b.bind = IpAddr::from([127, 0, 0, 1]);
        assert!(!a.engine_differs(&b));
        b.cache_gb = a.cache_gb + 1.0;
        assert!(a.engine_differs(&b));
    }
}
