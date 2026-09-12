//! The plugin trait every automation module implements, plus the shared
//! context handed to each one. There's no dynamic loading here (no dlopen,
//! no `.so` plugins) — "plugin-style" means each automation area is an
//! independent, self-contained module behind one interface, individually
//! enabled/disabled in config, not that it's loaded at runtime from outside
//! the binary. That's the right amount of abstraction for automations that
//! all ship in the same repo and get rebuilt together.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::config::AppConfig;
use crate::notify::Notifier;
use crate::state::StateStore;

/// Shared handles every plugin gets. Cheap to clone (an `Arc` internally
/// where it matters) since each plugin owns one for its whole lifetime.
#[derive(Clone)]
pub struct Context {
    pub config: Arc<AppConfig>,
    pub notifier: Notifier,
    pub state: StateStore,
    pub dry_run: bool,
}

/// One automation module. `run` is expected to loop forever internally
/// (a `tokio::time::interval` tick, a file-watch stream, whatever fits) —
/// there's no central scheduler ticking plugins from outside, because a
/// file-watch-driven plugin (theme sync) doesn't have a natural fixed
/// interval and forcing one on it would be the wrong shape. If `run`
/// returns (an error, a panic, an unexpected stream close) the daemon's
/// supervisor restarts it after a backoff — the same pattern vortexwall
/// uses for its own journalctl-tailing loop.
pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;

    fn run<'a>(
        &'a self,
        ctx: Context,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
}

/// Small helper so plugin `run` bodies can just `tick_forever(interval, ||
/// async { .. }).await` instead of hand-rolling a `loop { interval.tick()
/// ...}` each time. Errors are logged and swallowed — one bad tick
/// shouldn't kill the plugin, only a `return`/panic from outside this loop
/// does (which the top-level supervisor then restarts).
pub async fn tick_forever<F, Fut>(name: &str, period: Duration, mut f: F) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if let Err(e) = f().await {
            eprintln!("[{name}] tick error: {e:#}");
        }
    }
}
