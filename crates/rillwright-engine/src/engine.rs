//! The generation engine.
//!
//! `llama_context` and friends are raw pointers and must not be shared across threads, so the
//! model and context live on one dedicated worker thread. Callers talk to it over channels:
//! tokenize, generate (streamed), and save or restore a session.
//!
//! Each request prefills the prompt in `n_batch`-sized chunks, then samples one token at a
//! time. The sampler chain is top-p, then temperature, then a seeded `dist`, the same order as
//! `llama-server`'s defaults. A new chain is built for every request so a seed gives the same
//! tokens every time.

use std::{
    ffi::{CString, c_char},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::JoinHandle,
    time::Instant,
};

use rillwright_sys as sys;

use crate::{
    chat,
    error::{Error, Result},
    model::{CacheMode, Context, ContextParams, LoadOptions, Model},
    prefill::{self, PrefillSchedule},
};

#[derive(Clone, Debug)]
pub struct EngineOptions {
    pub load: LoadOptions,
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub threads: i32,
    /// Reuse cached KV entries for the longest prefix shared with the previous request.
    pub prefix_reuse: bool,
    pub swa_full: bool,
    pub flash_attn: crate::model::FlashAttn,
    /// Record the residual stream at each decode, for [`Engine::probe`].
    pub capture: bool,
}

/// One position of a probe: the state after the context so far, and what it predicts next.
#[derive(Clone, Debug)]
pub struct ProbeStep {
    /// The forced token that follows this position (none at the last step) and its logprob.
    pub forced: Option<(i32, f32, String)>,
    /// The most likely next tokens: id, logprob, text.
    pub top: Vec<(i32, f32, String)>,
    pub n_layers: usize,
    /// Residual stream of this position after each virtual layer, concatenated.
    pub hidden: Vec<f32>,
}

impl EngineOptions {
    pub fn new(load: LoadOptions) -> Self {
        EngineOptions {
            load,
            n_ctx: 32768,
            n_batch: 2048,
            n_ubatch: 512,
            threads: 8,
            // Off by default: under last_loop, reusing cached tokens changes greedy output
            // (see scripts/chat_acceptance.sh), so the prompt is re-evaluated unless asked.
            prefix_reuse: false,
            swa_full: false,
            flash_attn: crate::model::FlashAttn::Auto,
            capture: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub n_layer: i32,
    pub loops: u32,
    pub cache_mode: CacheMode,
    pub n_ctx: u32,
    pub n_ctx_train: i32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub chat_template: Option<String>,
    /// The user-turn marker (`<|im_start|>user\n`) as tokens, used to find where user messages start.
    pub user_turn: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenParams {
    /// Report the chosen token's logprob and the top-k alternatives for each token (0 = off).
    pub top_logprobs: u32,
    pub seed: u32,
    /// 0 selects greedy decoding.
    pub temp: f32,
    pub top_p: f32,
    pub max_tokens: u32,
    /// How the prompt is split into decode calls. The split changes the logits, so it is part of
    /// the result. See `prefill`.
    pub prefill: PrefillSchedule,
    /// Token positions where a user message starts, for the server's prefill breaks. Empty for a
    /// raw prompt. [`Engine::generate_chat`] fills this in for chat turns.
    pub message_starts: Vec<usize>,
}

impl Default for GenParams {
    fn default() -> Self {
        GenParams {
            seed: 42,
            temp: 1.0,
            top_p: 0.95,
            max_tokens: 8192,
            top_logprobs: 0,
            prefill: PrefillSchedule::default(),
            message_starts: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// End-of-generation token seen.
    Stop,
    /// `max_tokens` reached.
    Length,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenStats {
    pub finish: Finish,
    pub prompt_tokens: usize,
    /// Prompt tokens already in the KV cache from the previous request.
    pub reused_tokens: usize,
    /// Sizes of the prefill chunks actually decoded, in order.
    pub chunks: Vec<usize>,
    pub generated: usize,
    pub prompt_ms: f64,
    pub gen_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A chunk of decoded text. Chunks never split a UTF-8 character.
    Text(String),
    /// Emitted before the text of each generated token, only when `top_logprobs > 0`.
    Token {
        id: i32,
        logprob: f32,
        top: Vec<(i32, f32)>,
    },
    Done(GenStats),
}

pub struct Generation {
    rx: Receiver<Result<Event>>,
    cancel: Arc<AtomicBool>,
    finished: bool,
}

/// Cancels one generation from any thread, including while it is being iterated.
#[derive(Clone, Debug)]
pub struct CancelHandle(Arc<AtomicBool>);

impl CancelHandle {
    /// Stops after the current token (or prefill chunk). A `Done` event with `Finish::Cancelled` follows.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Generation {
    /// Stops after the current token. A `Done` event with `Finish::Cancelled` follows.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle(self.cancel.clone())
    }
}

/// Dropping a generation cancels it, so the worker stops within one token instead of running
/// to `max_tokens` and blocking the next request.
impl Drop for Generation {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Iterator for Generation {
    type Item = Result<Event>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.rx.recv() {
            Ok(Ok(ev)) => {
                if matches!(ev, Event::Done(_)) {
                    self.finished = true;
                }
                Some(Ok(ev))
            }
            Ok(Err(e)) => {
                self.finished = true;
                Some(Err(e))
            }
            Err(_) => {
                self.finished = true;
                Some(Err(Error::EngineStopped))
            }
        }
    }
}

enum Command {
    Tokenize {
        text: String,
        add_special: bool,
        parse_special: bool,
        reply: Sender<Result<Vec<i32>>>,
    },
    Generate {
        prompt: Vec<i32>,
        params: GenParams,
        cancel: Arc<AtomicBool>,
        events: Sender<Result<Event>>,
    },
    Probe {
        prompt: Vec<i32>,
        forced: Vec<i32>,
        top_k: usize,
        last_only: bool,
        reply: Sender<Result<Vec<ProbeStep>>>,
    },
    SaveSession {
        path: String,
        reply: Sender<Result<usize>>,
    },
    LoadSession {
        path: String,
        reply: Sender<Result<usize>>,
    },
    ClearSession {
        reply: Sender<()>,
    },
    Shutdown,
}

pub struct Engine {
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    pub info: ModelInfo,
}

impl Engine {
    /// Loads the model on the worker thread and returns once it is ready (or failed).
    pub fn start(opts: EngineOptions) -> Result<Engine> {
        let (tx, rx) = mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<ModelInfo>>();
        let thread = std::thread::Builder::new()
            .name("rillwright-engine".into())
            .spawn(move || worker(opts, rx, ready_tx))
            .map_err(|e| Error::Io {
                path: "rillwright-engine thread".into(),
                source: e,
            })?;
        let info = ready_rx.recv().map_err(|_| Error::EngineStopped)??;
        Ok(Engine {
            tx,
            thread: Some(thread),
            info,
        })
    }

    fn send(&self, cmd: Command) -> Result<()> {
        self.tx.send(cmd).map_err(|_| Error::EngineStopped)
    }

    pub fn tokenize(&self, text: &str, add_special: bool, parse_special: bool) -> Result<Vec<i32>> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Tokenize {
            text: text.to_string(),
            add_special,
            parse_special,
            reply,
        })?;
        rx.recv().map_err(|_| Error::EngineStopped)?
    }

    /// Starts generating from `prompt` (already tokenized). Events stream back as they're made.
    /// `params.message_starts` is used as given; use [`Engine::generate_chat`] for a chat turn.
    pub fn generate(&self, prompt: Vec<i32>, params: GenParams) -> Result<Generation> {
        let (events, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.send(Command::Generate {
            prompt,
            params,
            cancel: cancel.clone(),
            events,
        })?;
        Ok(Generation {
            rx,
            cancel,
            finished: false,
        })
    }

    /// Like [`Engine::generate`], for a rendered chat prompt. Finds the user-message starts in the
    /// tokens, so the prefill schedule breaks where `llama-server`'s chat endpoint does.
    pub fn generate_chat(&self, prompt: Vec<i32>, mut params: GenParams) -> Result<Generation> {
        params.message_starts = prefill::message_starts(&prompt, &self.info.user_turn);
        self.generate(prompt, params)
    }

    /// Decodes a rendered chat prompt with the usual prefill schedule, then feeds `forced` one
    /// token at a time. Returns one step per position from the last prompt token on: the residual
    /// stream there (when the engine was started with `capture`) and the next-token distribution.
    pub fn probe(
        &self,
        prompt: Vec<i32>,
        forced: Vec<i32>,
        top_k: usize,
        last_only: bool,
    ) -> Result<Vec<ProbeStep>> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Probe {
            prompt,
            forced,
            top_k,
            last_only,
            reply,
        })?;
        rx.recv().map_err(|_| Error::EngineStopped)?
    }

    /// Runs a generation to completion and returns the text and stats.
    pub fn generate_text(&self, prompt: Vec<i32>, params: GenParams) -> Result<(String, GenStats)> {
        let mut text = String::new();
        let mut stats = None;
        for ev in self.generate(prompt, params)? {
            match ev? {
                Event::Text(t) => text.push_str(&t),
                Event::Done(s) => stats = Some(s),
                Event::Token { .. } => {}
            }
        }
        Ok((text, stats.ok_or(Error::EngineStopped)?))
    }

    pub fn clear_session(&self) -> Result<()> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::ClearSession { reply })?;
        rx.recv().map_err(|_| Error::EngineStopped)
    }

    /// Writes the KV cache for the current conversation to `path`. Returns the bytes written.
    pub fn save_session(&self, path: &str) -> Result<usize> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::SaveSession {
            path: path.to_string(),
            reply,
        })?;
        rx.recv().map_err(|_| Error::EngineStopped)?
    }

    /// Restores a session written by `save_session`. Returns the number of cached tokens.
    pub fn load_session(&self, path: &str) -> Result<usize> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::LoadSession {
            path: path.to_string(),
            reply,
        })?;
        rx.recv().map_err(|_| Error::EngineStopped)?
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Worker {
    opts: EngineOptions,
    ctx: Context,
    /// Tokens currently held in the KV cache for sequence 0, in position order.
    kv: Vec<i32>,
}

fn worker(opts: EngineOptions, rx: Receiver<Command>, ready: Sender<Result<ModelInfo>>) {
    let setup = (|| -> Result<Worker> {
        let model: Rc<Model> = Model::load(&opts.load)?;
        let ctx = Context::new(
            model,
            &ContextParams {
                n_ctx: opts.n_ctx,
                n_batch: opts.n_batch,
                n_ubatch: opts.n_ubatch,
                n_threads: opts.threads,
                swa_full: opts.swa_full,
                flash_attn: opts.flash_attn,
                capture: opts.capture,
            },
        )?;
        Ok(Worker {
            opts: opts.clone(),
            ctx,
            kv: Vec::new(),
        })
    })();
    let mut worker = match setup {
        Ok(w) => w,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    match worker.info() {
        Ok(info) => {
            let _ = ready.send(Ok(info));
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    }

    for cmd in rx {
        match cmd {
            Command::Tokenize {
                text,
                add_special,
                parse_special,
                reply,
            } => {
                let _ = reply.send(tokenize_model(
                    worker.ctx.model(),
                    &text,
                    add_special,
                    parse_special,
                ));
            }
            Command::Generate {
                prompt,
                params,
                cancel,
                events,
            } => {
                if let Err(e) = worker.generate(prompt, &params, &cancel, &events) {
                    // A failed decode can leave the cache half-written, so drop it.
                    worker.ctx.clear_kv();
                    worker.kv.clear();
                    let _ = events.send(Err(e));
                }
            }
            Command::Probe {
                prompt,
                forced,
                top_k,
                last_only,
                reply,
            } => {
                let out = worker.probe(&prompt, &forced, top_k, last_only);
                worker.ctx.clear_kv();
                worker.kv.clear();
                let _ = reply.send(out);
            }
            Command::SaveSession { path, reply } => {
                let _ = reply.send(worker.save_session(&path));
            }
            Command::LoadSession { path, reply } => {
                let _ = reply.send(worker.load_session(&path));
            }
            Command::ClearSession { reply } => {
                worker.ctx.clear_kv();
                worker.kv.clear();
                let _ = reply.send(());
            }
            Command::Shutdown => break,
        }
    }
}

impl Worker {
    fn info(&self) -> Result<ModelInfo> {
        let m = self.ctx.model();
        let n_layer = m.n_layer();
        Ok(ModelInfo {
            n_layer,
            loops: self.opts.load.loops,
            cache_mode: self.opts.load.cache_mode,
            n_ctx: self.ctx.n_ctx(),
            n_ctx_train: m.n_ctx_train(),
            n_batch: self.opts.n_batch,
            n_ubatch: self.opts.n_ubatch,
            chat_template: m.chat_template(),
            user_turn: tokenize_model(m, chat::USER_TURN, false, true)?,
        })
    }

    fn generate(
        &mut self,
        prompt: Vec<i32>,
        params: &GenParams,
        cancel: &AtomicBool,
        events: &Sender<Result<Event>>,
    ) -> Result<()> {
        if prompt.is_empty() {
            return Err(Error::EmptyPrompt);
        }
        let n_ctx = self.ctx.n_ctx() as usize;
        if prompt.len() >= n_ctx {
            return Err(Error::PromptTooLong {
                prompt: prompt.len(),
                n_ctx: n_ctx as u32,
            });
        }

        // Keep the shared prefix, drop the rest. Always re-run at least the last prompt token,
        // because sampling needs its logits.
        let shared = if self.opts.prefix_reuse {
            common_prefix(&self.kv, &prompt)
        } else {
            0
        };
        let reused = shared.min(prompt.len() - 1);
        if reused < self.kv.len() {
            self.ctx.truncate_kv(reused);
            self.kv.truncate(reused);
        }

        let chunks = prefill::chunk_sizes(
            params.prefill,
            reused,
            prompt.len(),
            self.opts.n_batch as usize,
            self.opts.n_ubatch as usize,
            &params.message_starts,
        );
        let t0 = Instant::now();
        let mut at = reused;
        let mut decoded = Vec::with_capacity(chunks.len());
        for &size in &chunks {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let part = &prompt[at..at + size];
            self.ctx.decode(part)?;
            self.kv.extend_from_slice(part);
            at += size;
            decoded.push(size);
        }
        let prompt_ms = t0.elapsed().as_secs_f64() * 1e3;
        if decoded.len() < chunks.len() {
            // Cancelled during prefill. The cache holds exactly the tokens decoded so far.
            let stats = GenStats {
                finish: Finish::Cancelled,
                prompt_tokens: prompt.len(),
                reused_tokens: reused,
                chunks: decoded,
                generated: 0,
                prompt_ms,
                gen_ms: 0.0,
            };
            let _ = events.send(Ok(Event::Done(stats)));
            return Ok(());
        }

        let sampler = Sampler::new(params);
        let mut utf8 = Utf8Stream::default();
        let mut generated = 0usize;
        let t1 = Instant::now();
        let finish = loop {
            if cancel.load(Ordering::Relaxed) {
                break Finish::Cancelled;
            }
            if generated >= params.max_tokens as usize {
                break Finish::Length;
            }
            let tok = sampler.sample(self.ctx.as_ptr());
            if self.ctx.model().is_eog(tok) {
                break Finish::Stop;
            }
            // Budget check: this token must be decoded, so it needs a free cache slot.
            if self.kv.len() >= n_ctx {
                return Err(Error::ContextExhausted {
                    pos: self.kv.len(),
                    n_ctx: n_ctx as u32,
                });
            }
            if params.top_logprobs > 0 {
                let (logprob, top) =
                    token_probs(self.ctx.last_logits(), tok, params.top_logprobs as usize);
                if events
                    .send(Ok(Event::Token {
                        id: tok,
                        logprob,
                        top,
                    }))
                    .is_err()
                {
                    break Finish::Cancelled;
                }
            }
            let text = utf8.push(&self.piece(tok));
            if !text.is_empty() && events.send(Ok(Event::Text(text))).is_err() {
                // The caller dropped the stream: stop before this token enters the cache.
                break Finish::Cancelled;
            }
            self.ctx.decode(&[tok])?;
            self.kv.push(tok);
            generated += 1;
        };
        let tail = utf8.finish();
        if !tail.is_empty() {
            let _ = events.send(Ok(Event::Text(tail)));
        }
        let stats = GenStats {
            finish,
            prompt_tokens: prompt.len(),
            reused_tokens: reused,
            chunks: decoded,
            generated,
            prompt_ms,
            gen_ms: t1.elapsed().as_secs_f64() * 1e3,
        };
        let _ = events.send(Ok(Event::Done(stats)));
        Ok(())
    }

    fn probe(
        &mut self,
        prompt: &[i32],
        forced: &[i32],
        top_k: usize,
        last_only: bool,
    ) -> Result<Vec<ProbeStep>> {
        if prompt.is_empty() {
            return Err(Error::EmptyPrompt);
        }
        let n_ctx = self.ctx.n_ctx() as usize;
        if prompt.len() + forced.len() >= n_ctx {
            return Err(Error::PromptTooLong {
                prompt: prompt.len() + forced.len(),
                n_ctx: n_ctx as u32,
            });
        }
        self.ctx.clear_kv();
        self.kv.clear();
        let starts = prefill::message_starts(prompt, &tokenize_model(
            self.ctx.model(),
            chat::USER_TURN,
            false,
            true,
        )?);
        let chunks = prefill::chunk_sizes(
            PrefillSchedule::default(),
            0,
            prompt.len(),
            self.opts.n_batch as usize,
            self.opts.n_ubatch as usize,
            &starts,
        );
        let mut at = 0;
        for &size in &chunks {
            self.ctx.decode(&prompt[at..at + size])?;
            at += size;
        }
        let text = |w: &Worker, t: i32| String::from_utf8_lossy(&w.piece(t)).into_owned();
        let mut steps = Vec::with_capacity(forced.len() + 1);
        for i in 0..=forced.len() {
            let next = forced.get(i).copied();
            let (lp, top) = token_probs(self.ctx.last_logits(), next.unwrap_or(0), top_k);
            let (mut n_layers, mut hidden) = self.ctx.captured();
            if last_only && n_layers > 1 {
                // Keep only the final layer: the state the output weights read.
                let width = hidden.len() / n_layers;
                hidden.drain(..hidden.len() - width);
                n_layers = 1;
            }
            steps.push(ProbeStep {
                forced: next.map(|t| (t, lp, text(self, t))),
                top: top.into_iter().map(|(t, l)| (t, l, text(self, t))).collect(),
                n_layers,
                hidden,
            });
            if let Some(t) = next {
                self.ctx.decode(&[t])?;
            }
        }
        Ok(steps)
    }

    fn piece(&self, token: i32) -> Vec<u8> {
        let vocab = self.ctx.model().vocab();
        let mut buf = vec![0 as c_char; 64];
        let mut n = unsafe {
            sys::llama_token_to_piece(vocab, token, buf.as_mut_ptr(), buf.len() as i32, 0, true)
        };
        if n < 0 {
            buf = vec![0 as c_char; (-n) as usize];
            n = unsafe {
                sys::llama_token_to_piece(vocab, token, buf.as_mut_ptr(), buf.len() as i32, 0, true)
            };
        }
        buf[..n.max(0) as usize].iter().map(|&b| b as u8).collect()
    }

    fn save_session(&self, path: &str) -> Result<usize> {
        let cpath = CString::new(path).map_err(|_| Error::Session("path contains NUL".into()))?;
        let written = unsafe {
            sys::llama_state_seq_save_file(
                self.ctx.as_ptr(),
                cpath.as_ptr(),
                0,
                self.kv.as_ptr(),
                self.kv.len(),
            )
        };
        if written == 0 {
            return Err(Error::Session(format!("could not write {path}")));
        }
        Ok(written)
    }

    /// Replaces the cache with the one in `path`. The cache is cleared before the load, not after:
    /// clearing after would discard the restored state while `kv` still claimed it was there.
    fn load_session(&mut self, path: &str) -> Result<usize> {
        let cpath = CString::new(path).map_err(|_| Error::Session("path contains NUL".into()))?;
        self.ctx.clear_kv();
        self.kv.clear();
        let cap = self.ctx.n_ctx() as usize;
        let mut tokens = vec![0i32; cap];
        let mut count = 0usize;
        let ok = unsafe {
            sys::llama_state_seq_load_file(
                self.ctx.as_ptr(),
                cpath.as_ptr(),
                0,
                tokens.as_mut_ptr(),
                cap,
                &mut count,
            )
        };
        if ok == 0 {
            self.ctx.clear_kv();
            return Err(Error::Session(format!("could not read {path}")));
        }
        tokens.truncate(count);
        self.kv = tokens;
        Ok(self.kv.len())
    }
}

/// Log-probability of `chosen` and the `k` most likely tokens, from raw logits.
fn token_probs(logits: &[f32], chosen: i32, k: usize) -> (f32, Vec<(i32, f32)>) {
    use std::cmp::Ordering;
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    let sum: f64 = logits.iter().map(|&l| (l as f64 - max).exp()).sum();
    let lse = max + sum.ln();
    let lp = |i: usize| (logits[i] as f64 - lse) as f32;
    let k = k.clamp(1, logits.len());
    let mut idx: Vec<usize> = (0..logits.len()).collect();
    idx.select_nth_unstable_by(k - 1, |&a, &b| {
        logits[b].partial_cmp(&logits[a]).unwrap_or(Ordering::Equal)
    });
    let mut top = idx[..k].to_vec();
    top.sort_by(|&a, &b| logits[b].partial_cmp(&logits[a]).unwrap_or(Ordering::Equal));
    (
        lp(chosen as usize),
        top.into_iter().map(|i| (i as i32, lp(i))).collect(),
    )
}

fn common_prefix(a: &[i32], b: &[i32]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Tokenizes with a model's vocabulary alone; no context or GPU memory needed.
pub fn tokenize_model(
    model: &Model,
    text: &str,
    add_special: bool,
    parse_special: bool,
) -> Result<Vec<i32>> {
    let vocab = model.vocab();
    let bytes = text.as_bytes();
    let mut cap = bytes.len() + 16;
    loop {
        let mut out = vec![0i32; cap];
        let n = unsafe {
            sys::llama_tokenize(
                vocab,
                bytes.as_ptr() as *const c_char,
                bytes.len() as i32,
                out.as_mut_ptr(),
                cap as i32,
                add_special,
                parse_special,
            )
        };
        if n >= 0 {
            out.truncate(n as usize);
            return Ok(out);
        }
        cap = (-n) as usize;
        if cap > 1 << 24 {
            return Err(Error::Tokenize(bytes.len()));
        }
    }
}

/// Owns a sampler chain. Chain members are freed with the chain.
struct Sampler {
    ptr: *mut sys::llama_sampler,
}

impl Sampler {
    fn new(p: &GenParams) -> Sampler {
        unsafe {
            let chain = sys::llama_sampler_chain_init(sys::llama_sampler_chain_default_params());
            if p.temp <= 0.0 {
                sys::llama_sampler_chain_add(chain, sys::llama_sampler_init_greedy());
            } else {
                if p.top_p < 1.0 {
                    sys::llama_sampler_chain_add(chain, sys::llama_sampler_init_top_p(p.top_p, 1));
                }
                sys::llama_sampler_chain_add(chain, sys::llama_sampler_init_temp(p.temp));
                sys::llama_sampler_chain_add(chain, sys::llama_sampler_init_dist(p.seed));
            }
            Sampler { ptr: chain }
        }
    }

    fn sample(&self, ctx: *mut sys::llama_context) -> i32 {
        unsafe { sys::llama_sampler_sample(self.ptr, ctx, -1) }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        unsafe { sys::llama_sampler_free(self.ptr) }
    }
}

/// Turns a byte stream into text, holding back an incomplete UTF-8 sequence until it finishes.
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or_default());
                    match e.error_len() {
                        // Incomplete sequence at the end: wait for more bytes.
                        None => {
                            self.pending.drain(..valid);
                            break;
                        }
                        // Invalid bytes: replace them and carry on.
                        Some(n) => {
                            out.push('\u{FFFD}');
                            self.pending.drain(..valid + n);
                        }
                    }
                }
            }
        }
        out
    }

    fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        if rest.is_empty() {
            String::new()
        } else {
            String::from_utf8_lossy(&rest).into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_multibyte_char_is_held_back() {
        let mut s = Utf8Stream::default();
        let bytes = "é!".as_bytes(); // é is two bytes
        assert_eq!(s.push(&bytes[..1]), "");
        assert_eq!(s.push(&bytes[1..]), "é!");
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn utf8_invalid_bytes_become_replacement() {
        let mut s = Utf8Stream::default();
        assert_eq!(s.push(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
    }

    #[test]
    fn utf8_truncated_tail_is_flushed_lossily() {
        let mut s = Utf8Stream::default();
        assert_eq!(s.push(&[0xC3]), "");
        assert_eq!(s.finish(), "\u{FFFD}");
    }

    #[test]
    fn common_prefix_counts_matching_head() {
        assert_eq!(common_prefix(&[1, 2, 3, 4], &[1, 2, 9]), 2);
        assert_eq!(common_prefix(&[], &[1]), 0);
    }
}
