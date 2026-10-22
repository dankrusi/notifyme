use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Message, SmtpTransport, Transport};
use serde_json::{Value, json};

const USAGE: &str = "\
notifyme - send a notification by email or Telegram

  notifyme email \"message\" [--subject S] [--to a@b.com,c@d.com]
  notifyme telegram \"message\" [--to CHAT_ID]
  notifyme install    set up credentials, PATH and agent skills
  notifyme config     print the config file path

The message may also be piped via stdin. Without --to it goes to the configured default.";

const SKILL_MD: &str = include_str!("../skill/SKILL.md");
const AGENTS_MD: &str = include_str!("../AGENTS.md");

// Keys in the order they are written to the config file.
const KEYS: &[&str] = &[
    "EMAIL_TO",
    "SMTP_HOST",
    "SMTP_PORT",
    "SMTP_SECURITY",
    "SMTP_USERNAME",
    "SMTP_PASSWORD",
    "SMTP_FROM",
    "SMTP_FROM_NAME",
    "TELEGRAM_BOT_API_KEY",
    "TELEGRAM_TO",
];

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("notifyme: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    let mut cfg = Config::load()?;
    match args.cmd.as_str() {
        "email" | "telegram" => {
            let msg = args.message()?;
            if args.cmd == "email" {
                send_email(&cfg, &msg, &args.to, args.subject.as_deref())?;
            } else {
                send_telegram(&cfg, &msg, &args.to)?;
            }
            println!("sent");
            Ok(())
        }
        "install" => install(&mut cfg),
        "config" => {
            println!("{}", cfg.path.display());
            Ok(())
        }
        "" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

// ---------- args ----------

struct Args {
    cmd: String,
    words: Vec<String>,
    to: Vec<String>,
    subject: Option<String>,
}

impl Args {
    fn parse(mut it: impl Iterator<Item = String>) -> Result<Args> {
        let mut a = Args { cmd: String::new(), words: vec![], to: vec![], subject: None };
        while let Some(arg) = it.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (arg.clone(), None),
            };
            let mut value = |name: &str| inline.clone().or_else(|| it.next()).ok_or(format!("{name} needs a value"));
            match flag.as_str() {
                "--to" | "-t" => a.to.extend(split_list(&value("--to")?)),
                "--subject" | "-s" => a.subject = Some(value("--subject")?),
                "-h" | "--help" => a.cmd = "help".into(),
                _ if a.cmd.is_empty() => a.cmd = arg,
                _ => a.words.push(arg),
            }
        }
        Ok(a)
    }

    // Positional words joined, or stdin when none given (or "-").
    fn message(&self) -> Result<String> {
        let mut msg = self.words.join(" ");
        if (msg.is_empty() || msg == "-") && !io::stdin().is_terminal() {
            msg.clear();
            io::stdin().read_to_string(&mut msg).map_err(|e| e.to_string())?;
        }
        let msg = msg.trim().to_string();
        if msg.is_empty() || msg == "-" {
            return Err(format!("no message given\n\n{USAGE}"));
        }
        Ok(msg)
    }
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

// ---------- config ----------

struct Config {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn app_dir() -> PathBuf {
    home().join(".notifyme")
}

impl Config {
    fn load() -> Result<Config> {
        let path = std::env::var_os("NOTIFYME_CONFIG").map(PathBuf::from).unwrap_or_else(|| app_dir().join(".env"));
        let mut values = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let v = v.trim();
                    let v = match (v.chars().next(), v.chars().last()) {
                        (Some(q @ ('"' | '\'')), Some(e)) if q == e && v.len() >= 2 => &v[1..v.len() - 1],
                        _ => v,
                    };
                    values.insert(k.trim().to_string(), v.to_string());
                }
            }
        }
        Ok(Config { path, values })
    }

    // NOTIFYME_<KEY> env vars override the file.
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(format!("NOTIFYME_{key}"))
            .ok()
            .or_else(|| self.values.get(key).cloned())
            .filter(|v| !v.trim().is_empty())
    }

    fn need(&self, key: &str) -> Result<String> {
        self.get(key).ok_or_else(|| format!("{key} is not set in {} (run `notifyme install`)", self.path.display()))
    }

    fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut out = String::from("# notifyme config. Env vars NOTIFYME_<KEY> override these values.\n");
        let known = KEYS.iter().map(|k| k.to_string());
        let extra = self.values.keys().filter(|k| !KEYS.contains(&k.as_str())).cloned();
        for k in known.chain(extra) {
            if let Some(v) = self.values.get(&k) {
                out.push_str(&format!("{k}={v}\n"));
            }
        }
        std::fs::write(&self.path, out).map_err(|e| format!("cannot write {}: {e}", self.path.display()))
    }
}

// ---------- email ----------

fn send_email(cfg: &Config, msg: &str, to: &[String], subject: Option<&str>) -> Result<()> {
    let host = cfg.need("SMTP_HOST")?;
    let port: u16 = match cfg.get("SMTP_PORT") {
        Some(p) => p.parse().map_err(|_| format!("invalid SMTP_PORT '{p}'"))?,
        None => 465,
    };
    let security = cfg.get("SMTP_SECURITY").unwrap_or_else(|| if port == 465 { "tls" } else { "starttls" }.into());
    let username = cfg.get("SMTP_USERNAME");
    let from_addr = cfg.get("SMTP_FROM").or_else(|| username.clone()).ok_or("SMTP_FROM is not set (run `notifyme install`)")?;
    let from = match cfg.get("SMTP_FROM_NAME") {
        Some(name) => format!("{name} <{from_addr}>"),
        None => from_addr,
    };
    let recipients = if to.is_empty() { split_list(&cfg.need("EMAIL_TO")?) } else { to.to_vec() };
    let subject = subject.map(String::from).unwrap_or_else(|| default_subject(msg));

    let mut builder = Message::builder()
        .from(from.parse::<Mailbox>().map_err(|e| format!("invalid from address '{from}': {e}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN);
    for r in &recipients {
        builder = builder.to(r.parse::<Mailbox>().map_err(|e| format!("invalid address '{r}': {e}"))?);
    }
    let email = builder.body(msg.to_string()).map_err(|e| e.to_string())?;

    let tls = || TlsParameters::new(host.clone()).map_err(|e| e.to_string());
    let mut transport = SmtpTransport::builder_dangerous(&host).port(port).timeout(Some(Duration::from_secs(30)));
    transport = match security.to_lowercase().as_str() {
        "tls" | "ssl" => transport.tls(Tls::Wrapper(tls()?)),
        "starttls" => transport.tls(Tls::Required(tls()?)),
        "none" => transport,
        s => return Err(format!("invalid SMTP_SECURITY '{s}' (use tls, starttls or none)")),
    };
    if let Some(user) = username {
        transport = transport.credentials(Credentials::new(user, cfg.get("SMTP_PASSWORD").unwrap_or_default()));
    }
    transport.build().send(&email).map_err(|e| format!("email failed: {e}"))?;
    Ok(())
}

fn default_subject(msg: &str) -> String {
    let line = msg.lines().next().unwrap_or("").trim();
    if line.chars().count() > 78 { format!("{}...", line.chars().take(75).collect::<String>()) } else { line.to_string() }
}

// ---------- telegram ----------

fn telegram(token: &str, method: &str, body: Value) -> Result<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut resp = agent
        .post(&format!("https://api.telegram.org/bot{token}/{method}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("telegram request failed: {e}"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("telegram: unexpected response: {text}"))?;
    if v["ok"].as_bool() != Some(true) {
        return Err(format!("telegram: {}", v["description"].as_str().unwrap_or(&text)));
    }
    Ok(v["result"].clone())
}

fn send_telegram(cfg: &Config, msg: &str, to: &[String]) -> Result<()> {
    let token = cfg.need("TELEGRAM_BOT_API_KEY")?;
    let chats = if to.is_empty() { split_list(&cfg.need("TELEGRAM_TO")?) } else { to.to_vec() };
    // Telegram caps messages at 4096 chars.
    let chars: Vec<char> = msg.chars().collect();
    for chat in &chats {
        for part in chars.chunks(4000) {
            let text: String = part.iter().collect();
            telegram(&token, "sendMessage", json!({ "chat_id": chat, "text": text }))?;
        }
    }
    Ok(())
}

// ---------- install ----------

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line).ok();
    line.trim().to_string()
}

fn ask_yn(prompt: &str, default: bool) -> bool {
    let a = ask(&format!("{prompt} [{}] ", if default { "Y/n" } else { "y/N" })).to_lowercase();
    if a.is_empty() { default } else { a.starts_with('y') }
}

// Prompts for a key only if it is missing.
fn fill(cfg: &mut Config, key: &str, label: &str, default: Option<String>) {
    if cfg.get(key).is_some() {
        return;
    }
    let hint = default.as_deref().map(|d| format!(" [{d}]")).unwrap_or_default();
    let v = ask(&format!("  {label}{hint}: "));
    let v = if v.is_empty() { default.unwrap_or_default() } else { v };
    cfg.set(key, &v);
}

fn fill_secret(cfg: &mut Config, key: &str, label: &str) {
    if cfg.get(key).is_some() {
        return;
    }
    let prompt = format!("  {label}: ");
    let v = rpassword::prompt_password(&prompt).unwrap_or_else(|_| ask(&prompt));
    cfg.set(key, v.trim());
}

fn install(cfg: &mut Config) -> Result<()> {
    println!("notifyme install (config: {})\n", cfg.path.display());

    let email_ready = |c: &Config| c.get("SMTP_HOST").is_some() && c.get("EMAIL_TO").is_some();
    if !email_ready(cfg) && ask_yn("Set up email (SMTP)?", true) {
        fill(cfg, "SMTP_HOST", "SMTP host", None);
        fill(cfg, "SMTP_PORT", "SMTP port (465 = TLS, 587 = STARTTLS)", Some("465".into()));
        fill(cfg, "SMTP_USERNAME", "SMTP username", None);
        fill_secret(cfg, "SMTP_PASSWORD", "SMTP password");
        fill(cfg, "SMTP_FROM", "From address", cfg.get("SMTP_USERNAME"));
        fill(cfg, "SMTP_FROM_NAME", "From name", Some("notifyme".into()));
        fill(cfg, "EMAIL_TO", "Default recipient email", cfg.get("SMTP_FROM"));
        cfg.save()?;
    }

    let tg_ready = |c: &Config| c.get("TELEGRAM_BOT_API_KEY").is_some() && c.get("TELEGRAM_TO").is_some();
    if !tg_ready(cfg) && ask_yn("Set up Telegram?", true) {
        fill(cfg, "TELEGRAM_BOT_API_KEY", "Bot token (create a bot with @BotFather)", None);
        cfg.save()?;
        if cfg.get("TELEGRAM_TO").is_none() {
            match detect_chat(&cfg.need("TELEGRAM_BOT_API_KEY")?) {
                Ok(chat) => cfg.set("TELEGRAM_TO", &chat),
                Err(e) => println!("  {e}"),
            }
            cfg.save()?;
        }
    }

    for (cmd, ready) in [("email", email_ready(cfg)), ("telegram", tg_ready(cfg))] {
        if ready && ask_yn(&format!("Send a test {cmd}?"), true) {
            let msg = "notifyme is set up";
            let r = if cmd == "email" { send_email(cfg, msg, &[], None) } else { send_telegram(cfg, msg, &[]) };
            println!("  {}", r.map(|_| "sent".to_string()).unwrap_or_else(|e| e));
        }
    }

    install_path()?;
    install_skills()?;

    println!("\nFor other agents, add this to their AGENTS.md:\n\n{AGENTS_MD}");
    println!("Done. Edit {} to change settings.", cfg.path.display());
    Ok(())
}

fn detect_chat(token: &str) -> Result<String> {
    let me = telegram(token, "getMe", json!({}))?;
    let bot = me["username"].as_str().unwrap_or("your bot");
    loop {
        let a = ask(&format!("  Send any message to @{bot} in Telegram, then press Enter (or type a chat id): "));
        if !a.is_empty() {
            return Ok(a);
        }
        let updates = telegram(token, "getUpdates", json!({}))?;
        let chat = updates.as_array().into_iter().flatten().rev().find_map(|u| {
            let c = u.get("message").or_else(|| u.get("channel_post")).or_else(|| u.get("my_chat_member"))?.get("chat")?;
            let name = c["title"].as_str().or(c["first_name"].as_str()).unwrap_or("?").to_string();
            Some((c["id"].to_string(), name))
        });
        match chat {
            Some((id, name)) => {
                println!("  Found chat '{name}' ({id})");
                return Ok(id);
            }
            None => println!("  No messages found yet."),
        }
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "notifyme.exe" } else { "notifyme" }
}

fn which() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe_name())).find(|p| p.is_file())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn install_path() -> Result<()> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = app_dir().join("bin");
    let target = bin.join(exe_name());
    match which() {
        Some(found) if !same_file(&found, &target) || same_file(&current, &target) => {
            println!("Already on PATH: {}", found.display());
            return Ok(());
        }
        Some(_) => {
            if ask_yn(&format!("Update {} with this version?", target.display()), true) {
                std::fs::copy(&current, &target).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        None => {}
    }
    if !ask_yn(&format!("Copy notifyme to {} and add it to your PATH?", bin.display()), true) {
        return Ok(());
    }
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    if !same_file(&current, &target) {
        std::fs::copy(&current, &target).map_err(|e| e.to_string())?;
    }
    if cfg!(windows) {
        let dir = bin.display().to_string().replace('\'', "''");
        let script = format!(
            "$p=[Environment]::GetEnvironmentVariable('Path','User'); if (-not $p) {{ $p='' }}; \
             if (($p -split ';') -notcontains '{dir}') {{ [Environment]::SetEnvironmentVariable('Path', (($p.TrimEnd(';') + ';{dir}').TrimStart(';')), 'User') }}"
        );
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            println!("  Added to user PATH. Open a new terminal to use `notifyme`.");
        } else {
            println!("  Could not update PATH; add {} manually.", bin.display());
        }
    } else {
        println!("  Add this to your shell profile: export PATH=\"{}:$PATH\"", bin.display());
    }
    Ok(())
}

const MARK_START: &str = "<!-- notifyme -->";
const MARK_END: &str = "<!-- /notifyme -->";

// Inserts or replaces the notifyme block in an instructions file.
fn upsert_block(path: &Path) -> Result<()> {
    let block = format!("{MARK_START}\n{}{MARK_END}\n", AGENTS_MD);
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let out = match (text.find(MARK_START), text.find(MARK_END)) {
        (Some(s), Some(e)) if e > s => format!("{}{block}{}", &text[..s], text[e + MARK_END.len()..].trim_start_matches(['\r', '\n'])),
        _ if text.is_empty() => block,
        _ => format!("{}\n\n{block}", text.trim_end()),
    };
    std::fs::write(path, out).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn install_skills() -> Result<()> {
    let h = home();
    // (agent, its config dir, file to write, is a skill)
    let targets = [
        ("Claude Code", h.join(".claude"), h.join(".claude").join("skills").join("notifyme").join("SKILL.md"), true),
        ("Codex", h.join(".codex"), h.join(".codex").join("skills").join("notifyme").join("SKILL.md"), true),
        ("Gemini CLI", h.join(".gemini"), h.join(".gemini").join("GEMINI.md"), false),
    ];
    for (agent, dir, file, skill) in targets {
        if !dir.is_dir() || !ask_yn(&format!("Install for {agent} ({})?", file.display()), true) {
            continue;
        }
        if skill {
            std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&file, SKILL_MD).map_err(|e| e.to_string())?;
        } else {
            upsert_block(&file)?;
        }
        println!("  wrote {}", file.display());
    }
    Ok(())
}
