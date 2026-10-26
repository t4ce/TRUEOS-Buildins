
pub fn init() {
    use tracing_subscriber::filter::{EnvFilter, LevelFilter};

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).from_env_lossy(),
        )
        .init();
}

#[allow(unused_imports)]
pub use ::tracing::*;
