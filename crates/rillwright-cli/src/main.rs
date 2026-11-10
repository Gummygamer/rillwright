//! `rillwright`: command-line front end for the Dory harness.

use std::{
    io::{BufRead, Write, stderr, stdin, stdout},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rillwright_agent::{
    Agent, AgentConfig, AgentEvent, Reason, Registry, ToolCtx, TurnReport, backend::EngineBackend,
    policy::Policy, tools::ToolSet, workspace::Workspace,
};
use rillwright_engine::{
    CacheMode, Engine, EngineOptions, Event, GenParams, LoadOptions, PrefillSchedule,
    chat::{self, Message, ToolDef},
    model::Model,
    parse::{Piece, StreamParser},
    tokenize_model, verify,
};

mod agent_cmd;

#[derive(Parser)]
#[command(name = "rillwright", version = env!("DORY_VERSION"), about = "A local coding agent and desktop workspace, built in Rust")]
struct Cli {
    #[command(flatten)]
    load: LoadArgs,
    #[command(flatten)]
    gen_args: GenArgs,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct LoadArgs {
    /// Path to the GGUF file (its SHA-256 must match the pin). Defaults to RILLWRIGHT_MODEL (legacy DORY_MODEL).
    #[arg(long, global = true)]
    model: Option<PathBuf>,
    /// Recurrence loops, 1 to 4.
    #[arg(long, global = true, default_value_t = 2)]
    loops: u32,
    #[arg(long, global = true, value_enum, default_value_t = CacheModeArg::LastLoop)]
    cache_mode: CacheModeArg,
    /// Context size in tokens. 128K runs out of memory on the 6 GB card.
    #[arg(long, global = true, default_value_t = 32768)]
    ctx: u32,
    #[arg(long, global = true, default_value_t = 2048)]
    batch: u32,
    #[arg(long, global = true, default_value_t = 512)]
    ubatch: u32,
    /// Layers on the GPU; -1 means all.
    #[arg(long, global = true, default_value_t = -1, allow_negative_numbers = true)]
    gpu_layers: i32,
    #[arg(long, global = true, default_value_t = 8)]
    threads: i32,
    /// Keep a full-length cache for sliding-window layers (llama-server uses off).
    #[arg(long, global = true)]
    swa_full: bool,
    /// Flash attention: auto, on or off (llama-server uses auto).
    #[arg(long, global = true, value_enum, default_value_t = FlashArg::Auto)]
    flash_attn: FlashArg,
    /// Reuse cached KV for the prompt prefix shared with the previous request. Saves time in
    /// multi-turn chat, but under last_loop it can change greedy output. Off by default.
    #[arg(long, global = true)]
    prefix_reuse: bool,
}

#[derive(Args, Clone)]
struct GenArgs {
    #[arg(long, global = true, default_value_t = 42)]
    seed: u32,
    /// 0 selects greedy decoding.
    #[arg(
        long,
        global = true,
        default_value_t = 1.0,
        allow_negative_numbers = true
    )]
    temp: f32,
    #[arg(long, global = true, default_value_t = 0.95)]
    top_p: f32,
    #[arg(long, global = true, default_value_t = 8192)]
    max_tokens: u32,
    /// Render the prompt with thinking off.
    #[arg(long, global = true)]
    no_think: bool,
    /// JSON array of tools; only entries with an `exec` command can run.
    #[arg(long, global = true)]
    tools: Option<PathBuf>,
    /// Do not print reasoning to stderr.
    #[arg(long, global = true)]
    quiet_reasoning: bool,
    /// Record the logprob of each token and its top-k alternatives (used by `raw`).
    #[arg(long, global = true, default_value_t = 0)]
    top_logprobs: u32,
    /// How the prompt is split into decode calls. `server` matches llama-server (default); `single`
    /// decodes it in full batches. The split changes the output, so every result records it.
    #[arg(long, global = true, default_value = "server", value_parser = parse_prefill)]
    prefill: PrefillSchedule,
}

fn parse_prefill(s: &str) -> std::result::Result<PrefillSchedule, String> {
    s.parse()
}

#[derive(Clone, Copy, ValueEnum)]
enum FlashArg {
    Auto,
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum CacheModeArg {
    #[value(name = "last_loop", alias = "last-loop")]
    LastLoop,
    #[value(name = "per_loop", alias = "per-loop")]
    PerLoop,
}

impl From<CacheModeArg> for CacheMode {
    fn from(c: CacheModeArg) -> Self {
        match c {
            CacheModeArg::LastLoop => CacheMode::LastLoop,
            CacheModeArg::PerLoop => CacheMode::PerLoop,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Verify the checksum and print model metadata (no GPU memory used).
    Info,
    /// Render a case file ({messages, tools?, enable_thinking?}) to the exact prompt and token IDs, as JSON.
    Template { case: PathBuf },
    /// Answer one prompt and exit.
    Run {
        prompt: String,
        #[arg(long)]
        system: Option<String>,
    },
    /// Interactive multi-turn chat. Commands: /reset, /save PATH, /load PATH, /exit.
    Chat {
        #[arg(long)]
        system: Option<String>,
    },
    /// Generate for a case file and print the raw text plus its parsed fields, as JSON.
    /// Also checks that parsing one character at a time gives the same result.
    Raw {
        case: PathBuf,
        /// Decode with the chat-turn breaks that `/v1/chat/completions` uses, not the `/completion`
        /// schedule. Use it when comparing with the chat endpoint.
        #[arg(long)]
        chat_turn: bool,
    },
    /// Read the model's internal state. Each case file is {prompt} (rendered text, as written by
    /// RILLWRIGHT_DUMP_ROUNDS) or {messages, tools?, enable_thinking?}, plus an optional `forced`
    /// continuation that is fed token by token. For every position from the last prompt token on,
    /// writes the next-token logprobs to NAME.json and the residual stream after each layer to
    /// NAME.f32 (float32, shape [steps, layers, width]) in --out-dir.
    Probe {
        cases: Vec<PathBuf>,
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long, default_value_t = 10)]
        top: usize,
        /// Skip the residual stream and write only the logprobs.
        #[arg(long)]
        no_hidden: bool,
        /// Keep only the final layer of the residual stream (the state the output weights read).
        #[arg(long)]
        last_layer: bool,
    },
    /// Run the agent: the model reads and changes files and runs commands in a project folder,
    /// under permission rules. Give a task, or leave it out for an interactive session.
    Agent(agent_cmd::AgentArgs),
    /// Time prefill and decode for one configuration.
    Bench {
        #[arg(long, default_value_t = 512)]
        prompt_tokens: usize,
        #[arg(long, default_value_t = 128)]
        gen_tokens: u32,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let mut cli = Cli::parse();
    if cli.load.model.is_none() {
        cli.load.model = rillwright_app::config::model_env().map(PathBuf::from);
    }

    let model = cli.load.model_path()?;
    verify::verify_fork()?;
    eprintln!("checking model sha256 (about 8 s for 3.6 GB)...");
    verify::verify_model(model)?;

    match &cli.cmd {
        Cmd::Info => info(&cli.load),
        Cmd::Template { case } => template(&cli, case),
        Cmd::Run { prompt, system } => run(&cli, prompt, system.as_deref()),
        Cmd::Chat { system } => chat_repl(&cli, system.as_deref()),
        Cmd::Raw { case, chat_turn } => raw(&cli, case, *chat_turn),
        Cmd::Probe {
            cases,
            out_dir,
            top,
            no_hidden,
            last_layer,
        } => probe(&cli, cases, out_dir, *top, *no_hidden, *last_layer),
        Cmd::Bench {
            prompt_tokens,
            gen_tokens,
        } => bench(&cli, *prompt_tokens, *gen_tokens),
        Cmd::Agent(args) => {
            let code = agent_cmd::run(&cli, args)?;
            std::process::exit(code);
        }
    }
}

impl LoadArgs {
    /// The GGUF path from `--model` or `RILLWRIGHT_MODEL` (legacy `DORY_MODEL`).
    fn model_path(&self) -> Result<&Path> {
        self.model
            .as_deref()
            .context("no model given: pass --model or set RILLWRIGHT_MODEL (legacy DORY_MODEL) to the GGUF file")
    }
}

fn engine_options(load: &LoadArgs, vocab_only: bool) -> Result<EngineOptions> {
    let config = rillwright_app::config::LoadConfig {
        model: load.model_path()?.to_path_buf(),
        loops: load.loops,
        cache_mode: load.cache_mode.into(),
        context: load.ctx,
        batch: load.batch,
        micro_batch: load.ubatch,
        gpu_layers: load.gpu_layers,
        threads: load.threads,
        flash_attention: match load.flash_attn {
            FlashArg::Auto => rillwright_engine::model::FlashAttn::Auto,
            FlashArg::On => rillwright_engine::model::FlashAttn::On,
            FlashArg::Off => rillwright_engine::model::FlashAttn::Off,
        },
        prefix_reuse: load.prefix_reuse,
        swa_full: load.swa_full,
    };
    config
        .engine_options_with_vocab_only(vocab_only)
        .map_err(anyhow::Error::msg)
}

fn start_engine(cli: &Cli) -> Result<Engine> {
    eprintln!(
        "loading: loops={} cache={} ctx={} batch={} ubatch={} gpu_layers={}",
        cli.load.loops,
        CacheMode::from(cli.load.cache_mode).as_str(),
        cli.load.ctx,
        cli.load.batch,
        cli.load.ubatch,
        cli.load.gpu_layers
    );
    let engine = Engine::start(engine_options(&cli.load, false)?).context("starting the engine")?;
    let template = engine.info.chat_template.clone().unwrap_or_default();
    chat::check_template(&template)?;
    Ok(engine)
}

fn info(load: &LoadArgs) -> Result<()> {
    let model = Model::load(&LoadOptions {
        path: load.model_path()?.to_path_buf(),
        loops: load.loops,
        cache_mode: load.cache_mode.into(),
        n_gpu_layers: 0,
        vocab_only: true,
    })?;
    let template = model.chat_template().unwrap_or_default();
    chat::check_template(&template)?;
    println!("version:            {}", env!("DORY_VERSION"));
    println!("model:              {}", load.model_path()?.display());
    println!("sha256:             ok ({})", verify::PINNED_MODEL_SHA256);
    println!("fork commit:        ok ({})", verify::PINNED_FORK_COMMIT);
    println!("chat template:      ok (Dory format)");
    println!(
        "dory.block_count:   {}",
        model.meta("dory.block_count").unwrap_or_default()
    );
    println!(
        "dory.num_loops:     {} (GGUF default)",
        model.meta("dory.num_loops").unwrap_or_default()
    );
    println!(
        "dory.cache_mode:    {} (GGUF default)",
        model
            .meta("dory.recurrent_kv_cache_mode")
            .unwrap_or_default()
    );
    println!(
        "loops in use:       {} ({} virtual layers = 32 + 40 x loops), cache {}",
        load.loops,
        32 + 40 * load.loops,
        load.cache_mode.as_str_name()
    );
    Ok(())
}

impl CacheModeArg {
    fn as_str_name(self) -> &'static str {
        CacheMode::from(self).as_str()
    }
}

/// A render case: `{"messages": [...], "tools": [...], "enable_thinking": bool}`.
#[derive(serde::Deserialize)]
struct RenderCase {
    messages: Vec<Message>,
    #[serde(default)]
    tools: Vec<ToolDef>,
    enable_thinking: Option<bool>,
}

fn template(cli: &Cli, case_path: &PathBuf) -> Result<()> {
    let model = Model::load(&LoadOptions {
        path: cli.load.model_path()?.to_path_buf(),
        loops: cli.load.loops,
        cache_mode: cli.load.cache_mode.into(),
        n_gpu_layers: 0,
        vocab_only: true,
    })?;
    chat::check_template(&model.chat_template().unwrap_or_default())?;
    let case: RenderCase = serde_json::from_str(&std::fs::read_to_string(case_path)?)?;
    let thinking = case.enable_thinking.unwrap_or(!cli.gen_args.no_think);
    let prompt = chat::render(&case.messages, &case.tools, thinking)?;
    let tokens = tokenize_model(&model, &prompt, false, true)?;
    let out = serde_json::json!({ "prompt": prompt, "tokens": tokens });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn gen_params(gen_args: &GenArgs) -> GenParams {
    GenParams {
        seed: gen_args.seed,
        temp: gen_args.temp,
        top_p: gen_args.top_p,
        max_tokens: gen_args.max_tokens,
        top_logprobs: gen_args.top_logprobs,
        prefill: gen_args.prefill,
        message_starts: Vec::new(),
    }
}

/// The tools for `run` and `chat`: exactly the entries of the `--tools` file, if one was given.
fn load_tools(gen_args: &GenArgs) -> Result<Registry> {
    let set = match &gen_args.tools {
        Some(p) => ToolSet::load(p)?,
        None => ToolSet::default(),
    };
    let workspace =
        Workspace::new(&std::env::current_dir()?, &[]).context("opening the current directory")?;
    let mut registry = Registry::new(ToolCtx::new(workspace));
    set.register_into(&mut registry)?;
    Ok(registry)
}

fn vram_mib() -> Option<u64> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn run(cli: &Cli, prompt: &str, system: Option<&str>) -> Result<()> {
    let engine = start_engine(cli)?;
    let tools = load_tools(&cli.gen_args)?;
    let mut messages = Vec::new();
    if let Some(s) = system {
        messages.push(Message::new("system", s));
    }
    messages.push(Message::new("user", prompt));
    let report = run_turn(cli, &engine, &tools, &mut messages)?;
    println!();
    print_stats(&report);
    Ok(())
}

/// Runs one user turn through the agent loop and prints its events the way the CLI always has:
/// the answer on stdout, everything else on stderr.
fn run_turn(
    cli: &Cli,
    engine: &Engine,
    tools: &Registry,
    messages: &mut Vec<Message>,
) -> Result<TurnReport> {
    let backend = EngineBackend { engine };
    // run and chat: the --tools file is the allowlist, so nothing asks.
    let policy = Policy::allow_all();
    let agent = Agent {
        backend: &backend,
        tools,
        policy: &policy,
        config: AgentConfig::new(!cli.gen_args.no_think, gen_params(&cli.gen_args)),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let show_reasoning = !cli.gen_args.quiet_reasoning;
    let max_rounds = agent.config.max_rounds;
    let mut show = |ev: AgentEvent| print_event(ev, show_reasoning, max_rounds, false);
    Ok(agent.run_turn(messages, &mut show)?)
}

fn print_event(ev: AgentEvent, show_reasoning: bool, max_rounds: usize, verbose: bool) {
    match ev {
        AgentEvent::Reasoning(t) => {
            if show_reasoning {
                let _ = write!(stderr(), "{t}");
                let _ = stderr().flush();
            }
        }
        AgentEvent::Content(t) => {
            let _ = write!(stdout(), "{t}");
            let _ = stdout().flush();
        }
        AgentEvent::ToolCall { name, arguments } => {
            let _ = writeln!(
                stderr(),
                "\n[tool call] {name} {}",
                serde_json::Value::Object(arguments)
            );
        }
        AgentEvent::MalformedCall(_) => {
            let _ = writeln!(stderr(), "\n[warning] malformed tool call kept as text");
        }
        AgentEvent::ToolResult { text, .. } => {
            let shown = if verbose { text } else { truncate(&text, 200) };
            let _ = writeln!(stderr(), "[tool result] {shown}");
        }
        AgentEvent::RoundStats(_) | AgentEvent::MessageAdded(_) => {}
        AgentEvent::Compacted {
            stage,
            before,
            after,
        } => {
            let _ = writeln!(
                stderr(),
                "\n[context compacted: stage {stage}, {before} -> {after} tokens]"
            );
        }
        AgentEvent::Finished(Reason::RoundLimit) => {
            let _ = writeln!(stderr(), "\n[stopped after {max_rounds} tool rounds]");
        }
        AgentEvent::Finished(Reason::Answered) => {}
        AgentEvent::Finished(other) => {
            let _ = writeln!(stderr(), "\n[stopped: {}]", other.as_str());
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

fn print_stats(report: &TurnReport) {
    for s in &report.stats {
        let gen_tps = if s.gen_ms > 0.0 {
            s.generated as f64 / (s.gen_ms / 1e3)
        } else {
            0.0
        };
        let prompt_tps = if s.prompt_ms > 0.0 {
            (s.prompt_tokens - s.reused_tokens) as f64 / (s.prompt_ms / 1e3)
        } else {
            0.0
        };
        eprintln!(
            "[{:?}] prompt {} tokens ({} reused) at {:.0} tok/s, generated {} at {:.1} tok/s",
            s.finish, s.prompt_tokens, s.reused_tokens, prompt_tps, s.generated, gen_tps
        );
    }
}

fn chat_repl(cli: &Cli, system: Option<&str>) -> Result<()> {
    let engine = start_engine(cli)?;
    let tools = load_tools(&cli.gen_args)?;
    let mut messages: Vec<Message> = system
        .map(|s| vec![Message::new("system", s)])
        .unwrap_or_default();
    eprintln!("ready. /reset, /save PATH, /load PATH, /exit");
    let mut lines = stdin().lock().lines();
    loop {
        eprint!("you> ");
        let _ = stderr().flush();
        let Some(line) = lines.next() else { break };
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "/exit" {
            break;
        }
        if line == "/reset" {
            messages = system
                .map(|s| vec![Message::new("system", s)])
                .unwrap_or_default();
            engine.clear_session()?;
            eprintln!("[session cleared]");
            continue;
        }
        if let Some(path) = line.strip_prefix("/save ") {
            let bytes = engine.save_session(path.trim())?;
            std::fs::write(
                format!("{}.messages.json", path.trim()),
                serde_json::to_string_pretty(&messages)?,
            )?;
            eprintln!("[saved {bytes} bytes of KV cache]");
            continue;
        }
        if let Some(path) = line.strip_prefix("/load ") {
            let cached = engine.load_session(path.trim())?;
            messages = serde_json::from_str(&std::fs::read_to_string(format!(
                "{}.messages.json",
                path.trim()
            ))?)?;
            eprintln!("[loaded {cached} cached tokens]");
            continue;
        }
        messages.push(Message::new("user", line));
        print!("rillwright> ");
        let _ = stdout().flush();
        let report = run_turn(cli, &engine, &tools, &mut messages)?;
        println!();
        print_stats(&report);
    }
    Ok(())
}

/// Joins adjacent text pieces of the same kind so two parses can be compared.
fn merge_pieces(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        match (out.last_mut(), p) {
            (Some(Piece::Reasoning(a)), Piece::Reasoning(b)) => a.push_str(&b),
            (Some(Piece::Content(a)), Piece::Content(b)) => a.push_str(&b),
            (_, p) => out.push(p),
        }
    }
    out
}

#[derive(serde::Deserialize)]
struct ProbeCase {
    prompt: Option<String>,
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    tools: Vec<ToolDef>,
    enable_thinking: Option<bool>,
    #[serde(default)]
    forced: String,
}

fn probe(
    cli: &Cli,
    cases: &[PathBuf],
    out_dir: &PathBuf,
    top: usize,
    no_hidden: bool,
    last_layer: bool,
) -> Result<()> {
    use std::io::Write;
    let mut opts = engine_options(&cli.load, false)?;
    opts.capture = !no_hidden;
    let engine = Engine::start(opts).context("starting the engine")?;
    chat::check_template(&engine.info.chat_template.clone().unwrap_or_default())?;
    std::fs::create_dir_all(out_dir)?;
    for path in cases {
        let case: ProbeCase = serde_json::from_str(&std::fs::read_to_string(path)?)
            .with_context(|| format!("reading {}", path.display()))?;
        let prompt = match &case.prompt {
            Some(p) => p.clone(),
            None => chat::render(
                &case.messages,
                &case.tools,
                case.enable_thinking.unwrap_or(!cli.gen_args.no_think),
            )?,
        };
        let tokens = engine.tokenize(&prompt, false, true)?;
        let forced = if case.forced.is_empty() {
            Vec::new()
        } else {
            engine.tokenize(&case.forced, false, true)?
        };
        let steps = engine.probe(tokens.clone(), forced, top, last_layer)?;
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let n_layers = steps.first().map(|s| s.n_layers).unwrap_or(0);
        let width = steps
            .first()
            .map(|s| if s.n_layers > 0 { s.hidden.len() / s.n_layers } else { 0 })
            .unwrap_or(0);
        let json = serde_json::json!({
            "prompt_tokens": tokens.len(),
            "steps": steps.len(),
            "layers": n_layers,
            "width": width,
            "loops": cli.load.loops,
            "forced": steps.iter().map(|s| match &s.forced {
                Some((id, lp, text)) => serde_json::json!([id, lp, text]),
                None => serde_json::Value::Null,
            }).collect::<Vec<_>>(),
            "top": steps.iter().map(|s| s.top.iter()
                .map(|(id, lp, text)| serde_json::json!([id, lp, text])).collect::<Vec<_>>()
            ).collect::<Vec<_>>(),
        });
        std::fs::write(out_dir.join(format!("{name}.json")), json.to_string())?;
        if !no_hidden {
            let mut f = std::io::BufWriter::new(std::fs::File::create(out_dir.join(format!("{name}.f32")))?);
            for s in &steps {
                for v in &s.hidden {
                    f.write_all(&v.to_le_bytes())?;
                }
            }
            f.flush()?;
        }
        eprintln!("{name}: {} prompt tokens, {} steps, {n_layers} layers", tokens.len(), steps.len());
    }
    Ok(())
}

fn raw(cli: &Cli, case_path: &PathBuf, chat_turn: bool) -> Result<()> {
    let engine = start_engine(cli)?;
    let case: RenderCase = serde_json::from_str(&std::fs::read_to_string(case_path)?)?;
    let thinking = case.enable_thinking.unwrap_or(!cli.gen_args.no_think);
    let prompt = chat::render(&case.messages, &case.tools, thinking)?;
    let tokens = engine.tokenize(&prompt, false, true)?;
    let prompt_tokens = tokens.len();
    let schemas: std::collections::HashMap<String, serde_json::Value> = case
        .tools
        .iter()
        .map(|t| (t.name.clone(), t.parameters.clone()))
        .collect();

    let mut raw = String::new();
    let mut probs = Vec::new();
    let mut stats = None;
    let params = gen_params(&cli.gen_args);
    let generation = if chat_turn {
        engine.generate_chat(tokens, params)?
    } else {
        engine.generate(tokens, params)?
    };
    for ev in generation {
        match ev? {
            Event::Text(t) => raw.push_str(&t),
            Event::Token { id, logprob, top } => probs.push(serde_json::json!({
                "id": id,
                "logprob": logprob,
                "top": top.iter().map(|(i, lp)| serde_json::json!([i, lp])).collect::<Vec<_>>(),
            })),
            Event::Done(s) => stats = Some(s),
        }
    }
    let stats = stats.ok_or_else(|| anyhow::anyhow!("generation ended without a Done event"))?;

    let mut whole = StreamParser::new(thinking, schemas.clone());
    let mut pieces = whole.push(&raw);
    pieces.extend(whole.finish());

    let mut chunked = StreamParser::new(thinking, schemas);
    let mut chunk_pieces = Vec::new();
    let mut buf = [0u8; 4];
    for ch in raw.chars() {
        chunk_pieces.extend(chunked.push(ch.encode_utf8(&mut buf)));
    }
    chunk_pieces.extend(chunked.finish());
    let chunking_consistent = merge_pieces(pieces.clone()) == merge_pieces(chunk_pieces);

    let pieces = merge_pieces(pieces);
    let mut reasoning = String::new();
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut malformed = Vec::new();
    for p in pieces {
        match p {
            Piece::Reasoning(t) => reasoning.push_str(&t),
            Piece::Content(t) => content.push_str(&t),
            Piece::ToolCall { name, arguments } => {
                tool_calls.push(serde_json::json!({ "name": name, "arguments": arguments }))
            }
            Piece::MalformedToolCall(raw) => malformed.push(raw),
        }
    }
    let out = serde_json::json!({
        "prompt_tokens": prompt_tokens,
        "finish": format!("{:?}", stats.finish),
        "generated": stats.generated,
        "raw": raw,
        "reasoning": reasoning,
        "content": content,
        "tool_calls": tool_calls,
        "malformed": malformed,
        "chunking_consistent": chunking_consistent,
        "prefill": format!("{:?}", cli.gen_args.prefill),
        "chat_turn": chat_turn,
        "chunks": stats.chunks,
        "probs": probs,
    });
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

fn bench(cli: &Cli, prompt_tokens: usize, gen_tokens: u32) -> Result<()> {
    let engine = start_engine(cli)?;
    let vram = vram_mib();

    // Build a prompt of about `prompt_tokens` tokens from a repeated sentence.
    let unit = "The quick brown fox jumps over the lazy dog while the river runs south. ";
    let per_unit = engine.tokenize(unit, false, false)?.len().max(1);
    let reps = prompt_tokens.div_ceil(per_unit).max(1);
    let text = unit.repeat(reps);
    let mut tokens = engine.tokenize(&text, false, false)?;
    let ctx = cli.load.ctx as usize;
    if tokens.len() + gen_tokens as usize >= ctx {
        bail!(
            "prompt ({}) plus gen tokens ({gen_tokens}) must fit in ctx {ctx}",
            tokens.len()
        );
    }
    tokens.truncate(prompt_tokens.max(1));

    let mut params = gen_params(&cli.gen_args);
    params.max_tokens = gen_tokens;
    let t = Instant::now();
    let (_, stats) = engine.generate_text(tokens, params)?;
    let wall = t.elapsed().as_secs_f64();
    let prefill_tps = (stats.prompt_tokens - stats.reused_tokens) as f64 / (stats.prompt_ms / 1e3);
    let decode_tps = stats.generated as f64 / (stats.gen_ms / 1e3);
    println!(
        "loops={} cache={} ctx={} batch={} prompt={} gen={} vram_mib={} prefill={:?} chunks={} prefill_tok_s={:.0} decode_tok_s={:.1} wall_s={:.2}",
        cli.load.loops,
        CacheMode::from(cli.load.cache_mode).as_str(),
        cli.load.ctx,
        cli.load.batch,
        stats.prompt_tokens,
        stats.generated,
        vram.map(|v| v.to_string()).unwrap_or_else(|| "n/a".into()),
        cli.gen_args.prefill,
        stats
            .chunks
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join("+"),
        prefill_tps,
        decode_tps,
        wall
    );
    let _ = stdout().flush();
    Ok(())
}
