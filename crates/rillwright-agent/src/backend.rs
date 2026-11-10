//! Text generation behind the agent loop.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use rillwright_engine::{Engine, Event, GenParams, GenStats, Result};

/// Something that turns a rendered prompt into streamed text.
pub trait Backend {
    /// Context size in tokens.
    fn n_ctx(&self) -> usize;

    /// Tokens the prompt takes, with special tokens as the model will see them.
    fn count_tokens(&self, prompt: &str) -> Result<usize>;

    /// Generates for `prompt`, calling `on_event` with each `Text` and `Token` event in order, and
    /// returns the stats (which carry the `Done` information).
    /// Setting `cancel` stops generation after the current token; the stats then say `Cancelled`.
    fn generate(
        &self,
        prompt: &str,
        params: &GenParams,
        cancel: &Arc<AtomicBool>,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<GenStats>;
}

/// The real model, through the engine's worker thread.
pub struct EngineBackend<'a> {
    pub engine: &'a Engine,
}

impl Backend for EngineBackend<'_> {
    fn n_ctx(&self) -> usize {
        self.engine.info.n_ctx as usize
    }

    fn count_tokens(&self, prompt: &str) -> Result<usize> {
        Ok(self.engine.tokenize(prompt, false, true)?.len())
    }

    fn generate(
        &self,
        prompt: &str,
        params: &GenParams,
        cancel: &Arc<AtomicBool>,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<GenStats> {
        let tokens = self.engine.tokenize(prompt, false, true)?;
        // RILLWRIGHT_DUMP_ROUNDS=DIR writes each round's exact prompt and raw output there, so a
        // round can be replayed with `rillwright probe`.
        let dump_dir = std::env::var_os("RILLWRIGHT_DUMP_ROUNDS").filter(|d| !d.is_empty());
        let mut dump = dump_dir.as_ref().map(|_| String::new());
        let generation = self.engine.generate_chat(tokens, params.clone())?;
        let handle = generation.cancel_handle();
        let finished = AtomicBool::new(false);
        let mut stats = None;
        let mut failure = None;
        std::thread::scope(|scope| {
            // The engine only sees a cancel between tokens, and this loop blocks on events, so a
            // watcher forwards the flag to the engine while the stream is being read.
            scope.spawn(|| {
                while !finished.load(Ordering::Relaxed) {
                    if cancel.load(Ordering::Relaxed) {
                        handle.cancel();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            for ev in generation {
                match ev {
                    Ok(Event::Done(s)) => stats = Some(s),
                    Ok(other) => {
                        if let (Some(out), Event::Text(t)) = (dump.as_mut(), &other) {
                            out.push_str(t);
                        }
                        on_event(other)
                    }
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            finished.store(true, Ordering::Relaxed);
        });
        if let (Some(dir), Some(output)) = (dump_dir, dump) {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
            let body = serde_json::json!({ "prompt": prompt, "output": output });
            let _ = std::fs::write(dir.join(format!("r{n:03}.json")), body.to_string());
        }
        if let Some(e) = failure {
            return Err(e);
        }
        stats.ok_or(rillwright_engine::Error::EngineStopped)
    }
}
