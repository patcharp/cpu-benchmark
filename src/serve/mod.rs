//! `cpubench serve`: a small web UI to start benchmark runs, watch them, open
//! reports and compare two of them. Self-contained (no CDN) so it works on an
//! offline server.
//!
//! Safety model: one run at a time; the run is a child process built from a
//! validated argument list (never a shell string); the output directory is
//! chosen by the operator, not the form; report files are served from a strict
//! name whitelist; loopback-only by default; any non-loopback bind requires a
//! token unless explicitly disabled.

mod job;
mod validate;

use crate::model::Report;
use crate::report::{self, compare::CompareOpts, render};
use crate::{Res, build_info, compare_files, io, load, now_unix, sysinfo};
use job::Phase;
use serde_json::{Value, json};
use std::io::{Cursor, Read};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use validate::{Env, RunRequest, safe_label, safe_report_file, validate_run};

const UI_HTML: &str = include_str!("ui.html");
const UI_JS: &str = include_str!("app.js");
const MAX_BODY: u64 = 64 * 1024;
const WORKERS: usize = 4;
const CSP_UI: &str = "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const CSP_REPORT: &str = "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub struct Options {
    pub bind: String,
    pub out: PathBuf,
    pub token: Option<String>,
    pub no_auth: bool,
}

struct Ctx {
    out: PathBuf,
    token: Option<String>,
    loopback: bool,
    hostname: String,
    usable: usize,
    allowed: Vec<usize>,
    system: Value,
    job: job::Shared,
}

type Resp = Response<Cursor<Vec<u8>>>;

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("ASCII header")
}

fn reply(status: u16, ctype: &str, body: Vec<u8>) -> Resp {
    let mut r = Response::from_data(body).with_status_code(StatusCode(status));
    for (k, v) in [
        ("Content-Type", ctype),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
    ] {
        r.add_header(header(k, v));
    }
    r
}

fn json_reply(status: u16, v: &Value) -> Resp {
    reply(status, "application/json; charset=utf-8", v.to_string().into_bytes())
}

fn err(status: u16, msg: &str) -> Resp {
    json_reply(status, &json!({ "error": msg }))
}

fn with_csp(mut r: Resp, csp: &str) -> Resp {
    r.add_header(header("Content-Security-Policy", csp));
    r
}

fn hdr(req: &Request, name: &'static str) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string())
}

/// Constant-time equality so a token cannot be guessed byte by byte.
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn cookie_token(cookie_header: &str) -> Option<&str> {
    cookie_header.split(';').find_map(|p| p.trim().strip_prefix("cb_token="))
}

/// With no token and a loopback bind, only loopback host *names* are accepted;
/// this stops DNS-rebinding pages from driving the API through the browser.
/// The port is ignored on purpose: an ssh tunnel (`-L 9000:127.0.0.1:8080`)
/// legitimately presents a different port than the one we listen on.
fn host_allowed(host: &str) -> bool {
    let name = if host.starts_with('[') {
        host.find(']').map_or(host, |i| &host[..=i])
    } else {
        host.rsplit_once(':').map_or(host, |(n, _)| n)
    };
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|h| h.eq_ignore_ascii_case(name))
}

/// Browsers always send Origin on cross-site POSTs; if present it must be us.
fn origin_ok(origin: Option<&str>, host: Option<&str>) -> bool {
    match (origin, host) {
        (None, _) => true,
        (Some(o), Some(h)) => o == format!("http://{h}"),
        (Some(_), None) => false,
    }
}

pub fn random_token() -> Res<String> {
    let mut buf = [0u8; 16];
    let mut f = io(std::fs::File::open("/dev/urandom"), "opening /dev/urandom")?;
    io(f.read_exact(&mut buf), "reading /dev/urandom")?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn run(opts: Options) -> Res<()> {
    let addrs: Vec<_> = io(opts.bind.to_socket_addrs(), &format!("bad --bind '{}'", opts.bind))?.collect();
    let loopback = !addrs.is_empty() && addrs.iter().all(|a| a.ip().is_loopback());
    let port = addrs.first().map_or(0, |a| a.port());

    let token = match (&opts.token, loopback, opts.no_auth) {
        (Some(t), _, _) if t.len() >= 8 => Some(t.clone()),
        (Some(_), _, _) => return Err("--token must be at least 8 characters".into()),
        (None, false, false) => Some(random_token()?),
        _ => None,
    };

    io(std::fs::create_dir_all(&opts.out), &format!("creating {}", opts.out.display()))?;
    let system = sysinfo::collect();
    let hostname = system.hostname.clone();
    let static_info = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "hostname": hostname,
        "system": system,
        "build": build_info(),
        "kernels": crate::kernels::all().iter().map(|k| json!({"id": k.id, "category": k.category, "desc": k.desc, "unit": k.unit, "multi": k.multi, "heavy": k.heavy})).collect::<Vec<_>>(),
        "auth": token.is_some(),
    });
    let ctx = Arc::new(Ctx {
        out: opts.out.clone(),
        token: token.clone(),
        loopback,
        hostname,
        usable: system.usable_parallelism,
        allowed: crate::sys::allowed_cpus(),
        system: static_info,
        job: job::new_shared(opts.out.clone()),
    });

    let server = Server::http(opts.bind.as_str()).map_err(|e| format!("cannot listen on {}: {e}", opts.bind))?;
    let server = Arc::new(server);
    // 0.0.0.0 / :: is not a URL anyone can open; show the machine's name instead.
    let shown = if addrs.first().is_some_and(|a| a.ip().is_unspecified()) { format!("{}:{port}", ctx.hostname) } else { opts.bind.clone() };
    eprintln!("cpubench {} listening on {}  (reports in {})", env!("CARGO_PKG_VERSION"), opts.bind, opts.out.display());
    match (&token, loopback) {
        (Some(t), _) => eprintln!("Open  http://{shown}/?token={t}   (the token is required; keep it private)"),
        (None, true) => eprintln!("Open  http://{shown}/   (loopback only; from another machine use: ssh -L {port}:127.0.0.1:{port} <user>@<this host>)"),
        (None, false) => eprintln!("WARNING: listening on a non-loopback address WITHOUT authentication (--no-auth). Anyone who can reach this port can start CPU-heavy runs."),
    }

    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (server, ctx) = (server.clone(), ctx.clone());
            thread::spawn(move || {
                while let Ok(req) = server.recv() {
                    handle(req, &ctx);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

fn split_url(url: &str) -> (&str, &str) {
    url.split_once('?').unwrap_or((url, ""))
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

fn handle(mut req: Request, ctx: &Ctx) {
    let resp = route(&mut req, ctx);
    let _ = req.respond(resp);
}

fn route(req: &mut Request, ctx: &Ctx) -> Resp {
    let host = hdr(req, "Host");
    if ctx.token.is_none() && ctx.loopback && !host.as_deref().is_some_and(host_allowed) {
        return err(403, "host not allowed (use http://127.0.0.1 or http://localhost)");
    }
    let url = req.url().to_string();
    let (path, query) = split_url(&url);
    let method = req.method().clone();

    if let Some(tok) = &ctx.token {
        let by_cookie = hdr(req, "Cookie").as_deref().and_then(cookie_token).is_some_and(|c| ct_eq(c, tok));
        let by_bearer = hdr(req, "Authorization").as_deref().and_then(|a| a.strip_prefix("Bearer ")).is_some_and(|b| ct_eq(b, tok));
        if !by_cookie && !by_bearer {
            if method == Method::Get && path == "/" && query_param(query, "token").is_some_and(|q| ct_eq(q, tok)) {
                let mut r = reply(302, "text/plain; charset=utf-8", Vec::new());
                r.add_header(header("Location", "/"));
                r.add_header(header("Set-Cookie", &format!("cb_token={tok}; HttpOnly; SameSite=Strict; Path=/")));
                return r;
            }
            return reply(401, "text/plain; charset=utf-8", b"Authentication required: open the URL printed by `cpubench serve` (it contains ?token=...).".to_vec());
        }
    }

    if method == Method::Post {
        let json_ct = hdr(req, "Content-Type").is_some_and(|c| c.to_ascii_lowercase().starts_with("application/json"));
        if !json_ct {
            return err(415, "POST requires Content-Type: application/json");
        }
        if !origin_ok(hdr(req, "Origin").as_deref(), host.as_deref()) {
            return err(403, "cross-origin request refused");
        }
    }

    match (&method, path) {
        (Method::Get, "/") => with_csp(reply(200, "text/html; charset=utf-8", UI_HTML.as_bytes().to_vec()), CSP_UI),
        (Method::Get, "/app.js") => reply(200, "text/javascript; charset=utf-8", UI_JS.as_bytes().to_vec()),
        (Method::Get, "/favicon.ico") => reply(204, "image/x-icon", Vec::new()),
        (Method::Get, "/api/system") => json_reply(200, &ctx.system),
        (Method::Get, "/api/status") => json_reply(200, &status(ctx)),
        (Method::Get, "/api/reports") => json_reply(200, &reports(ctx)),
        (Method::Post, "/api/run") => start_run(req, ctx),
        (Method::Post, "/api/cancel") => {
            if job::cancel(&ctx.job) { json_reply(200, &json!({"cancelled": true})) } else { err(409, "no run is in progress") }
        }
        (Method::Post, "/api/compare") => compare(req, ctx),
        (Method::Get, p) if p.starts_with("/reports/") => report_file(&p["/reports/".len()..], ctx),
        (_, "/" | "/api/system" | "/api/status" | "/api/reports" | "/api/run" | "/api/cancel" | "/api/compare") => err(405, "method not allowed"),
        _ => err(404, "not found"),
    }
}

fn read_body(req: &mut Request) -> Result<String, Resp> {
    let mut body = String::new();
    req.as_reader().take(MAX_BODY + 1).read_to_string(&mut body).map_err(|_| err(400, "unreadable request body"))?;
    if body.len() as u64 > MAX_BODY {
        return Err(err(413, "request body too large"));
    }
    Ok(body)
}

fn start_run(req: &mut Request, ctx: &Ctx) -> Resp {
    let body = match read_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let rr: RunRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => return err(400, &format!("invalid JSON: {e}")),
    };
    let out = ctx.out.clone();
    let exists = move |l: &str| out.join(format!("{l}.json")).exists() || job::partial_path(&out, l).exists();
    let env = Env { usable_parallelism: ctx.usable, allowed_cpus: &ctx.allowed, hostname: &ctx.hostname, now_unix: now_unix(), label_exists: &exists };
    let v = match validate_run(&rr, &env) {
        Ok(v) => v,
        Err(e) => return err(400, &e),
    };
    match job::start(&ctx.job, &v.label, &v.args) {
        Ok(()) => json_reply(202, &json!({"label": v.label, "args": v.args})),
        Err(e) => err(409, &e),
    }
}

fn status(ctx: &Ctx) -> Value {
    let v = job::view(&ctx.job);
    let total = crate::kernels::all().len();
    let mut results: Vec<crate::model::KernelResult> = Vec::new();
    if v.phase == Phase::Running {
        results = job::read_partial(&ctx.out, &v.label);
        if results.is_empty() {
            // The child may have just finished (final report written, partial file removed)
            // before the reaper noticed; use the final report so progress never jumps back to 0.
            results = load(&ctx.out.join(format!("{}.json", v.label))).map(|r| r.results).unwrap_or_default();
        }
    } else if v.phase == Phase::Done {
        results = load(&ctx.out.join(format!("{}.json", v.label))).map(|r| r.results).unwrap_or_default();
    }
    let current = (v.phase == Phase::Running).then(|| crate::kernels::all().get(results.len()).map(|k| k.id)).flatten();
    let rows: Vec<Value> = results
        .iter()
        .map(|r| json!({
            "id": r.id, "unit": r.unit, "status": r.status, "note": r.note,
            "single": r.single.as_ref().map(|m| m.median), "multi": r.multi.as_ref().map(|m| m.median),
            "cv_single": r.single.as_ref().map(|m| m.cv_pct), "cv_multi": r.multi.as_ref().map(|m| m.cv_pct),
        }))
        .collect();
    // Partial files nobody is working on: a run that died (server restart, crash, power loss).
    let interrupted: Vec<Value> = std::fs::read_dir(&ctx.out)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let label = name.strip_suffix(".partial.json")?.to_string();
            (v.phase != Phase::Running || v.label != label).then(|| json!({"label": label, "kernels_done": job::read_partial(&ctx.out, &label).len()}))
        })
        .collect();
    json!({
        "state": v.phase.name(), "label": v.label, "elapsed_secs": v.elapsed_secs,
        "done": results.len(), "total": total, "current": current,
        "results": rows, "error": v.error, "log_tail": v.log_tail, "interrupted": interrupted,
    })
}

fn reports(ctx: &Ctx) -> Value {
    let mut items: Vec<(u64, Value)> = Vec::new();
    let mut comparisons: Vec<String> = Vec::new();
    for e in std::fs::read_dir(&ctx.out).into_iter().flatten().flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        if name.starts_with("compare-") && name.ends_with(".html") {
            comparisons.push(name);
            continue;
        }
        let Some((stem, "json")) = safe_report_file(&name) else { continue };
        match load(&e.path()) {
            Ok(r) => items.push((r.started_unix, summary(stem, &r))),
            Err(msg) => items.push((0, json!({"name": stem, "error": msg}))),
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    comparisons.sort();
    json!({ "reports": items.into_iter().map(|i| i.1).collect::<Vec<_>>(), "comparisons": comparisons })
}

fn summary(stem: &str, r: &Report) -> Value {
    json!({
        "name": stem, "label": r.label, "cpu": r.system.cpu_model, "arch": r.system.arch, "profile": r.profile,
        "threads": r.config.threads, "started_unix": r.started_unix, "libc": r.build.libc,
        "score_single": r.scores.single, "score_multi": r.scores.multi, "warnings": r.warnings.len(),
    })
}

fn compare(req: &mut Request, ctx: &Ctx) -> Resp {
    let body = match read_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return err(400, &format!("invalid JSON: {e}")),
    };
    let label = |k: &str| v.get(k).and_then(Value::as_str).and_then(safe_label).map(str::to_string);
    let (Some(a), Some(b)) = (label("a"), label("b")) else { return err(400, "choose two reports (a and b)") };
    if a == b {
        return err(400, "choose two different reports");
    }
    let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0);
    let opts = CompareOpts { cost_a: num("cost_a"), cost_b: num("cost_b"), watts_a: num("watts_a"), watts_b: num("watts_b") };
    match compare_files(&ctx.out.join(format!("{a}.json")), &ctx.out.join(format!("{b}.json")), &ctx.out, &opts) {
        Ok((_, stem, _)) => json_reply(200, &json!({"html": format!("/reports/{stem}.html"), "md": format!("/reports/{stem}.md")})),
        Err(e) => err(400, &e),
    }
}

fn report_file(name: &str, ctx: &Ctx) -> Resp {
    let Some((stem, ext)) = safe_report_file(name) else { return err(404, "not found") };
    let (ctype, csp) = match ext {
        "html" => ("text/html; charset=utf-8", Some(CSP_REPORT)),
        "json" => ("application/json; charset=utf-8", None),
        _ => ("text/markdown; charset=utf-8", None),
    };
    let wrap = |body: Vec<u8>| {
        let mut r = reply(200, ctype, body);
        if ext != "html" {
            r.add_header(header("Content-Disposition", &format!("inline; filename=\"{stem}.{ext}\"")));
        }
        match csp {
            Some(c) => with_csp(r, c),
            None => r,
        }
    };
    if let Ok(bytes) = std::fs::read(ctx.out.join(name)) {
        return wrap(bytes);
    }
    // Only the JSON was copied here (e.g. from another machine): render the view on demand.
    if ext != "json" {
        if let Ok(rep) = load(&ctx.out.join(format!("{stem}.json"))) {
            let doc = report::run::build_doc(&rep);
            return wrap(if ext == "html" { render::html(&doc) } else { render::markdown(&doc) }.into_bytes());
        }
    }
    err(404, "not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq("abc12345", "abc12345"));
        assert!(!ct_eq("abc12345", "abc12346"));
        assert!(!ct_eq("abc", "abcd"));
    }

    #[test]
    fn cookie_parsing() {
        assert_eq!(cookie_token("a=1; cb_token=XYZ; b=2"), Some("XYZ"));
        assert_eq!(cookie_token("a=1"), None);
        assert_eq!(cookie_token("xcb_token=nope"), None);
    }

    #[test]
    fn rebinding_host_check() {
        assert!(host_allowed("127.0.0.1:8080"));
        assert!(host_allowed("LOCALHOST:8080"));
        assert!(host_allowed("[::1]:8080"));
        assert!(host_allowed("127.0.0.1"), "no port is fine");
        assert!(host_allowed("127.0.0.1:9000"), "ssh -L may use another local port");
        assert!(host_allowed("localhost:9000"));
        assert!(!host_allowed("evil.example.com:8080"));
        assert!(!host_allowed("localhost.evil.com:8080"));
        assert!(!host_allowed("127.0.0.1.evil.com"));
        assert!(!host_allowed("[::2]:8080"));
        assert!(!host_allowed(""));
    }

    #[test]
    fn csrf_origin_check() {
        assert!(origin_ok(None, Some("127.0.0.1:8080")), "curl sends no Origin");
        assert!(origin_ok(Some("http://127.0.0.1:8080"), Some("127.0.0.1:8080")));
        assert!(!origin_ok(Some("http://evil.example"), Some("127.0.0.1:8080")));
        assert!(!origin_ok(Some("http://127.0.0.1:8080"), None));
    }

    #[test]
    fn url_and_query_parsing() {
        assert_eq!(split_url("/a?x=1&token=T"), ("/a", "x=1&token=T"));
        assert_eq!(query_param("x=1&token=T", "token"), Some("T"));
        assert_eq!(query_param("xtoken=T", "token"), None);
        assert_eq!(split_url("/plain"), ("/plain", ""));
    }

    #[test]
    fn random_tokens_differ_and_are_long() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
