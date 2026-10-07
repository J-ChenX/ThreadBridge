//! Native fleet orchestration. SSH receives a fixed executable command and bounded JSON.
use crate::native_remote::{self, DB_BYTES, TABLES};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use clap::{Args, Subcommand};
use flate2::read::ZlibDecoder;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::process::{Child, Command};
use uuid::Uuid;
pub const MAX_RESPONSE: usize = 48 * 1024 * 1024;
pub const MAX_REQUEST: usize = 256 * 1024;
pub const LOCAL_UNITS: [&str; 3] = [
    "threadbridge-phone-hub.service",
    "threadbridge-phone-inbox.service",
    "threadbridge-phone-capture.service",
];
#[derive(Args)]
pub struct FleetArgs {
    #[arg(long, default_value = "local/fleet/fleet.json")]
    pub config: PathBuf,
    #[command(subcommand)]
    pub action: FleetAction,
}
#[derive(Subcommand)]
pub enum FleetAction {
    Deploy {
        name: Option<String>,
    },
    Install,
    Start,
    Stop,
    Restart,
    Status,
    Run {
        name: String,
    },
    Proxy {
        name: String,
        #[arg(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Probe,
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().with_context(|| format!("missing_{key}"))
}
fn validate_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn validate_ssh(target: &str) -> bool {
    let Some((user, address)) = target.split_once('@') else {
        return false;
    };
    !user.is_empty()
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && address
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.octets()[0] == 100)
}
pub fn load(path: &Path) -> Result<Value> {
    let c: Value = serde_json::from_slice(&fs::read(path)?)?;
    let hosts = c["hosts"].as_object().context("hosts_invalid")?;
    ensure!((1..=4).contains(&hosts.len()), "host_count");
    for (name, host) in hosts {
        ensure!(validate_name(name), "host_name");
        if host["local"] == true {
            continue;
        }
        ensure!(validate_ssh(string(host, "ssh")?), "tailnet_ssh_target");
        ensure!(
            matches!(string(host, "platform")?, "linux" | "windows"),
            "platform"
        );
        ensure!(
            !string(host, "helper")?.ends_with(".py"),
            "native_helper_path_required"
        );
    }
    Ok(c)
}
pub fn atomic(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.exists() {
        fs::create_dir_all(parent)?;
        private(parent, 0o700)?;
    }
    let tmp = parent.join(format!(
        ".{}-{}.next",
        path.file_name()
            .context("atomic_filename")?
            .to_string_lossy(),
        Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        replace(&tmp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result
}
pub fn replace(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
        }
        let a: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
        let b: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
        if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0x1 | 0x8) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(windows))]
    fs::rename(source, destination)?;
    Ok(())
}
fn private(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}
pub fn status_file(config: &Path, name: &str) -> PathBuf {
    config
        .parent()
        .unwrap_or(Path::new("."))
        .join(name)
        .join("status.json")
}
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
fn encoded_ps(script: &str) -> String {
    let data: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
        STANDARD.encode(data)
    )
}
fn remote_config(host: &Value) -> Result<String> {
    if let Some(s) = host["remote_config"].as_str() {
        return Ok(s.to_owned());
    }
    let helper = string(host, "helper")?;
    let split = if host["platform"] == "windows" {
        '\\'
    } else {
        '/'
    };
    let folder = helper
        .rsplit_once(split)
        .context("helper_absolute_path_required")?
        .0;
    Ok(format!("{folder}{split}remote.json"))
}
pub fn command(host: &Value) -> Result<String> {
    ensure!(
        !string(host, "helper")?.ends_with(".py"),
        "native_helper_path_required"
    );
    let args = [
        string(host, "helper")?.to_owned(),
        "remote-capture".into(),
        "--config".into(),
        remote_config(host)?,
    ];
    ensure!(
        !args.iter().any(|s| s.contains(['\0', '\r', '\n'])),
        "remote_command_invalid"
    );
    match string(host,"platform")?{"linux"=>Ok(args.iter().map(|s|shell_quote(s)).collect::<Vec<_>>().join(" ")),"windows"=>Ok(encoded_ps(&format!("$ErrorActionPreference='Stop';$OutputEncoding=New-Object System.Text.UTF8Encoding($false);[Console]::OutputEncoding=$OutputEncoding;$p=[Console]::In.ReadToEnd();$p | & {};exit $LASTEXITCODE",args.iter().map(|s|ps_quote(s)).collect::<Vec<_>>().join(" ")))),_=>bail!("platform")}
}
fn ssh_command(host: &Value, cmd: &str) -> Result<Command> {
    ensure!(validate_ssh(string(host, "ssh")?), "tailnet_ssh_target");
    let mut c = Command::new("ssh");
    c.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "ServerAliveCountMax=1",
        "--",
        string(host, "ssh")?,
        cmd,
    ]);
    Ok(c)
}
pub async fn ssh(host: &Value, cmd: &str, data: &[u8], timeout: Duration) -> Result<Vec<u8>> {
    native_remote::bounded_process(ssh_command(host, cmd)?, data, timeout, MAX_RESPONSE, 65536)
        .await
}
/// ASCII escaping makes Windows PowerShell 5.1's stdin encoding irrelevant.
pub fn ascii_json(value: &Value) -> Result<Vec<u8>> {
    let text = serde_json::to_string(value)?;
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c)
        } else {
            for word in c.encode_utf16(&mut [0; 2]) {
                out.push_str(&format!("\\u{word:04x}"));
            }
        }
    }
    Ok(out.into_bytes())
}
pub async fn rpc(host: &Value, request: &Value, timeout: Duration) -> Result<Value> {
    let encoded = ascii_json(request)?;
    ensure!(encoded.len() <= MAX_REQUEST, "rpc_request_limit");
    let raw = ssh(host, &command(host)?, &encoded, timeout).await?;
    let response: Value = serde_json::from_slice(&raw)?;
    ensure!(response["schema"] == 1, "rpc_schema");
    Ok(response)
}
pub fn validate_snapshot(raw: &str, destination: &Path, prefix: &str) -> Result<()> {
    let compressed = STANDARD.decode(raw)?;
    let mut decoder = ZlibDecoder::new(compressed.as_slice());
    let mut data = Vec::new();
    decoder.by_ref().take(DB_BYTES + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= DB_BYTES && decoder.total_in() == compressed.len() as u64,
        "snapshot_uncompressed_limit"
    );
    let tmp = destination.with_extension("download");
    atomic(&tmp, &data)?;
    let result = (|| -> Result<()> {
        let c = Connection::open_with_flags(&tmp, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        c.execute_batch("PRAGMA journal_mode=DELETE;")?;
        ensure!(
            c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? == "ok",
            "snapshot_corrupt"
        );
        let mut s = c.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
        let tables = s
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<BTreeSet<_>, _>>()?;
        ensure!(
            TABLES[..4].iter().all(|t| tables.contains(*t)),
            "snapshot_schema"
        );
        drop(s);
        c.execute_batch("CREATE TABLE IF NOT EXISTS captured_projects(thread_id TEXT PRIMARY KEY,project_path TEXT NOT NULL)")?;
        let mut s = c.prepare("SELECT thread_id,turn_id,reply FROM captured_replies")?;
        for row in s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (native, turn, body) = row?;
            Uuid::parse_str(&native)?;
            Uuid::parse_str(&turn)?;
            ensure!(body.len() <= 512 * 1024, "snapshot_reply_limit");
        }
        drop(s);
        c.execute(
            "UPDATE captured_replies SET title=? || substr(title,1,160)",
            [format!("{prefix} · ")],
        )?;
        drop(c);
        replace(&tmp, destination)
    })();
    let _ = fs::remove_file(&tmp);
    result
}
pub fn offline(c: &Value, host: &Value) -> Result<()> {
    let mut db = Connection::open(string(c, "hub_db")?)?;
    db.busy_timeout(Duration::from_secs(3))?;
    let tx = db.transaction()?;
    let id = string(host, "host_id")?;
    tx.execute("UPDATE devices SET last_seen=0 WHERE id=?", [id])?;
    tx.execute(
        "UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE host=?",
        [id],
    )?;
    tx.execute("UPDATE threads SET can_send=0 WHERE host=?", [id])?;
    tx.commit()?;
    Ok(())
}
pub fn queue_snapshot(source: &Path, destination: &Path, blocked: &[String]) -> Result<()> {
    let tmp = destination.with_extension("next");
    if tmp.exists() {
        fs::remove_file(&tmp)?;
    }
    native_remote::backup(source, &tmp)?;
    let mut db = Connection::open(&tmp)?;
    db.execute_batch("PRAGMA journal_mode=DELETE;")?;
    let tx = db.transaction()?;
    for table in TABLES
        .into_iter()
        .chain(["captured_images", "captured_visible_messages"])
    {
        if !tx.query_row(
            "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get::<_, bool>(0),
        )? {
            continue;
        }
        for native in blocked {
            tx.execute(&format!("DELETE FROM {table} WHERE thread_id=?"), [native])?;
        }
    }
    tx.commit()?;
    drop(db);
    private(&tmp, 0o600)?;
    replace(&tmp, destination)
}
pub fn quarantine(c: &Value, host: &Value, blocked: &[String]) -> Result<()> {
    let mut db = Connection::open(string(c, "hub_db")?)?;
    db.busy_timeout(Duration::from_secs(3))?;
    let tx = db.transaction()?;
    for native in blocked {
        tx.execute(
            "UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE host=? AND native=?",
            params![string(host, "host_id")?, native],
        )?;
        tx.execute(
            "UPDATE threads SET can_send=0 WHERE host=? AND native=?",
            params![string(host, "host_id")?, native],
        )?;
    }
    tx.commit()?;
    Ok(())
}
fn strings(value: &Value) -> Result<Vec<String>> {
    value
        .as_array()
        .context("blocked_threads_invalid")?
        .iter()
        .map(|v| Ok(v.as_str().context("blocked_thread_invalid")?.to_owned()))
        .collect()
}
async fn close(c: &Value, host: &Value, children: &mut Vec<Child>) -> Result<()> {
    for child in children.iter_mut() {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        let _ = child.start_kill();
    }
    for child in children.iter_mut() {
        if tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
    children.clear();
    offline(c, host)
}
fn excluded_ids(c: &Value, id: &str) -> Result<Vec<String>> {
    let db = Connection::open(string(c, "hub_db")?)?;
    let mut s=db.prepare("SELECT native FROM capture_targets ct JOIN tombstones t ON t.thread=ct.thread AND t.message='' WHERE ct.host=?")?;
    let rows = s
        .query_map([id], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
async fn cycle(
    c: &Value,
    name: &str,
    config_path: &Path,
    revision: &mut Option<String>,
    children: &mut Vec<Child>,
    queue_mode: &mut Option<bool>,
    blocked_threads: &mut Option<BTreeSet<String>>,
) -> Result<()> {
    let host = &c["hosts"][name];
    let directory = config_path.parent().unwrap_or(Path::new(".")).join(name);
    fs::create_dir_all(&directory)?;
    let source = directory.join("replies.sqlite");
    let writable = directory.join("queue.sqlite");
    let excluded = excluded_ids(c, string(host, "host_id")?)?;
    if let Some(local) = c["hosts"]
        .as_object()
        .context("hosts_invalid")?
        .values()
        .find(|h| h["local"] == true)
    {
        let local_excluded = excluded_ids(c, string(local, "host_id")?)?;
        if !local_excluded.is_empty() {
            let local_capture = c["local_capture_db"]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("local/notify-capture/replies.sqlite"));
            crate::collection::purge(&local_capture, &local_excluded)?;
        }
    }
    let response = rpc(
        host,
        &json!({"op":"snapshot","revision":revision,"excluded":excluded}),
        Duration::from_secs(if revision.is_none() { 45 } else { 10 }),
    )
    .await?;
    let enabled = host["verified_version"]
        .as_str()
        .is_some_and(|v| !v.is_empty() && response["version"] == v)
        && response["overflow"] == false;
    if queue_mode.is_some_and(|q| q != enabled) {
        close(c, host, children).await?;
    }
    *queue_mode = Some(enabled);
    if response["changed"] == true {
        let blocked = strings(&response["blocked_threads"])?;
        let blocked_set = blocked.iter().cloned().collect::<BTreeSet<_>>();
        if blocked_threads
            .as_ref()
            .is_some_and(|prior| *prior != blocked_set)
        {
            close(c, host, children).await?;
        }
        validate_snapshot(string(&response, "database")?, &source, name)?;
        let health = STANDARD.decode(string(&response, "health")?)?;
        ensure!(health.len() == 131072, "health_size");
        atomic(
            &PathBuf::from(format!("{}.health", source.display())),
            &health,
        )?;
        queue_snapshot(&source, &writable, &blocked)?;
        quarantine(c, host, &blocked)?;
        *blocked_threads = Some(blocked_set);
        *revision = Some(string(&response, "revision")?.into());
    }
    atomic(
        &status_file(config_path, name),
        &serde_json::to_vec(
            &json!({"online":true,"last_success":native_remote::now(),"version":response["version"],"queue":enabled,"blocked_threads":response["blocked_threads"],"revision":revision}),
        )?,
    )?;
    let mut exited = false;
    for child in children.iter_mut() {
        if child.try_wait()?.is_some() {
            exited = true;
        }
    }
    if exited {
        close(c, host, children).await?;
    }
    if children.is_empty() {
        let binary = string(c, "binary")?;
        let db = string(c, "hub_db")?;
        let id = string(host, "host_id")?;
        let mut sync = Command::new(binary);
        sync.arg("capture-sync")
            .arg("--db")
            .arg(db)
            .arg("--capture-db")
            .arg(&source)
            .arg("--host")
            .arg(id)
            .arg("--all-captured")
            .kill_on_drop(true);
        children.push(sync.spawn()?);
        let mut bridge = Command::new(binary);
        bridge
            .arg("capture-bridge")
            .arg("--db")
            .arg(db)
            .arg("--capture-db")
            .arg(if enabled { &writable } else { &source })
            .arg("--host")
            .arg(id)
            .kill_on_drop(true);
        if enabled {
            bridge
                .arg("--codex")
                .arg(directory.join("codex-proxy"))
                .arg("--allow-queue")
                .arg("--verified-version")
                .arg(string(host, "verified_version")?);
        }
        children.push(bridge.spawn()?);
    }
    Ok(())
}
pub async fn run_host(c: &Value, name: &str, config_path: &Path) -> Result<()> {
    let host = &c["hosts"][name];
    ensure!(
        host.is_object() && host["local"] != true,
        "remote_host_required"
    );
    let mut revision = None;
    let mut children = Vec::new();
    let mut queue_mode = None;
    let mut blocked = None;
    let mut stop = Box::pin(async {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! {r=tokio::signal::ctrl_c()=>r?,_=term.recv()=>{}}
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await?;
        Ok::<(), anyhow::Error>(())
    });
    loop {
        let started = Instant::now();
        let attempt = tokio::select! {r=&mut stop=>{r?;break},r=cycle(c,name,config_path,&mut revision,&mut children,&mut queue_mode,&mut blocked)=>r};
        if attempt.is_err() {
            let _ = close(c, host, &mut children).await;
            atomic(
                &status_file(config_path, name),
                &serde_json::to_vec(
                    &json!({"online":false,"checked_at":native_remote::now(),"error":"connection_or_capture_unavailable"}),
                )?,
            )?;
            eprintln!("{name}: unavailable; cached replies retained, queue disabled");
            revision = None;
        }
        let delay = Duration::from_secs(5)
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(500));
        tokio::select! {r=&mut stop=>{r?;break},_=tokio::time::sleep(delay)=>{}}
    }
    close(c, host, &mut children).await?;
    atomic(
        &status_file(config_path, name),
        &serde_json::to_vec(
            &json!({"online":false,"checked_at":native_remote::now(),"stopped":true}),
        )?,
    )?;
    Ok(())
}
pub fn proxy_request(host: &Value, state: &Value, args: &[String]) -> Result<Value> {
    if args == ["--version"] {
        return Ok(json!({"op":"version"}));
    }
    if args.len() == 2 && args[0] == "threadbridge-create" {
        ensure!(
            state["online"] == true
                && native_remote::now() - state["last_success"].as_f64().unwrap_or(0.0) <= 10.0,
            "remote_not_fresh"
        );
        let mut request: Value = serde_json::from_str(&args[1])?;
        ensure!(request["op"] == "create", "proxy_arguments");
        request["expected_version"] = host["verified_version"].clone();
        return Ok(request);
    }
    ensure!(
        args.len() == 5 && args[0] == "queue" && args[1] == "--thread" && args[3] == "--message",
        "proxy_arguments"
    );
    Uuid::parse_str(&args[2])?;
    ensure!(
        state["online"] == true
            && native_remote::now() - state["last_success"].as_f64().unwrap_or(0.0) <= 10.0,
        "remote_not_fresh"
    );
    ensure!(
        !strings(&state["blocked_threads"])
            .unwrap_or_default()
            .contains(&args[2]),
        "target_capture_unconfirmed"
    );
    Ok(
        json!({"op":"queue","thread":args[2],"text":args[4],"expected_version":host["verified_version"]}),
    )
}
pub async fn proxy(c: &Value, name: &str, config_path: &Path, args: &[String]) -> Result<()> {
    let host = &c["hosts"][name];
    let state = if args == ["--version"] {
        Value::Null
    } else {
        serde_json::from_slice(&fs::read(status_file(config_path, name))?)?
    };
    let request = proxy_request(host, &state, args)?;
    let response = rpc(
        host,
        &request,
        Duration::from_secs(if args == ["--version"] {
            4
        } else if args.first().is_some_and(|a| a == "threadbridge-create") {
            40
        } else {
            12
        }),
    )
    .await?;
    if args.first().is_some_and(|a| a == "threadbridge-create") {
        std::io::stdout().write_all(&serde_json::to_vec(&response)?)?;
    } else {
        std::io::stdout().write_all(string(&response, "stdout")?.as_bytes())?;
    }
    Ok(())
}
async fn systemctl(args: &[&str]) -> Result<Vec<u8>> {
    let mut c = Command::new("systemctl");
    c.arg("--user").args(args);
    native_remote::bounded_process(c, &[], Duration::from_secs(30), 65536, 65536).await
}
fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    if source.is_dir() {
        fs::create_dir_all(dest)?;
        for e in fs::read_dir(source)? {
            let e = e?;
            ensure!(
                !e.file_type()?.is_symlink(),
                "service_backup_symlink_refused"
            );
            copy_tree(&e.path(), &dest.join(e.file_name()))?;
        }
    } else {
        fs::copy(source, dest)?;
    }
    Ok(())
}
fn unit_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}
pub fn service_template(root: &Path, binary: &Path, config: &Path) -> String {
    // WorkingDirectory is a single literal path, unlike ExecStart's quoted words.
    let working_directory = root.to_string_lossy().replace('%', "%%");
    format!("[Unit]\nDescription=ThreadBridge remote capture %i\nPartOf=threadbridge.target\nAfter=threadbridge-phone-hub.service\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={} fleet --config {} run %i\nRestart=on-failure\nRestartSec=5\nUMask=0077\nMemoryHigh=128M\nMemoryMax=256M\nKillMode=control-group\n",working_directory,unit_quote(&binary.to_string_lossy()),unit_quote(&config.to_string_lossy()))
}
pub async fn install(c: &Value, config: &Path) -> Result<()> {
    ensure!(cfg!(unix), "systemd_requires_unix");
    let root = std::env::current_dir()?.canonicalize()?;
    let config = config.canonicalize()?;
    let home = std::env::var_os("HOME").context("home_unavailable")?;
    let units = PathBuf::from(home).join(".config/systemd/user");
    fs::create_dir_all(&units)?;
    let backup = root.join("local/backups").join(format!(
        "fleet-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        Uuid::new_v4()
    ));
    fs::create_dir_all(&backup)?;
    private(&backup, 0o700)?;
    native_remote::backup(Path::new(string(c, "hub_db")?), &backup.join("hub.sqlite"))?;
    // Save every affected fragment/drop-in and current local fragment before writing units.
    let mut fragments = Vec::new();
    for unit in LOCAL_UNITS
        .into_iter()
        .chain(["threadbridge.target", "threadbridge-remote@.service"])
    {
        if units.join(unit).exists() {
            copy_tree(&units.join(unit), &backup.join(unit))?;
        }
        let dropin = format!("{unit}.d");
        if units.join(&dropin).exists() {
            copy_tree(&units.join(&dropin), &backup.join(&dropin))?;
        }
        if LOCAL_UNITS.contains(&unit) {
            let raw = systemctl(&["show", unit, "-p", "FragmentPath", "--value"]).await?;
            let fragment = String::from_utf8(raw)?.trim().to_owned();
            ensure!(!fragment.is_empty(), "service_fragment_missing");
            fs::copy(&fragment, backup.join(format!("{unit}.fragment")))?;
            fragments.push((unit, fragment));
        }
    }
    for (unit, fragment) in fragments {
        if !units.join(unit).exists() {
            fs::copy(fragment, units.join(unit))?;
        }
        let d = units.join(format!("{unit}.d"));
        fs::create_dir_all(&d)?;
        atomic(
            &d.join("80-fleet.conf"),
            b"[Unit]\nPartOf=threadbridge.target\n",
        )?;
    }
    let names = c["hosts"]
        .as_object()
        .context("hosts_invalid")?
        .iter()
        .filter(|(_, h)| h["local"] != true)
        .map(|(n, _)| n.to_owned())
        .collect::<Vec<_>>();
    let wants = LOCAL_UNITS
        .into_iter()
        .map(str::to_owned)
        .chain(
            names
                .iter()
                .map(|n| format!("threadbridge-remote@{n}.service")),
        )
        .collect::<Vec<_>>()
        .join(" ");
    let target=format!("[Unit]\nDescription=ThreadBridge four-computer service\nWants={wants}\nAfter=network-online.target\n\n[Install]\nWantedBy=default.target\n");
    atomic(&units.join("threadbridge.target"), target.as_bytes())?;
    let binary = Path::new(string(c, "binary")?).canonicalize()?;
    atomic(
        &units.join("threadbridge-remote@.service"),
        service_template(&root, &binary, &config).as_bytes(),
    )?;
    for name in names {
        let directory = config.parent().unwrap().join(&name);
        fs::create_dir_all(&directory)?;
        let wrapper = format!(
            "#!/bin/sh\nexec {} fleet --config {} proxy {} -- \"$@\"\n",
            shell_quote(&binary.to_string_lossy()),
            shell_quote(&config.to_string_lossy()),
            shell_quote(&name)
        );
        let path = directory.join("codex-proxy");
        atomic(&path, wrapper.as_bytes())?;
        private(&path, 0o700)?;
    }
    systemctl(&["daemon-reload"]).await?;
    systemctl(&["enable", "threadbridge.target"]).await?;
    println!(
        "service definitions and consistent Hub backup: {}",
        backup.display()
    );
    Ok(())
}
fn binary_platform(path: &Path) -> Result<&'static str> {
    let mut f = fs::File::open(path)?;
    let mut magic = [0; 4];
    f.read_exact(&mut magic)?;
    if magic == *b"\x7fELF" {
        Ok("linux")
    } else if magic[..2] == *b"MZ" {
        Ok("windows")
    } else {
        bail!("remote_binary_format")
    }
}
pub fn deployment_binary(c: &Value, host: &Value) -> Result<PathBuf> {
    let path = host["remote_binary"]
        .as_str()
        .map(PathBuf::from)
        .or_else(|| {
            if host["platform"] == "linux" {
                c["binary"].as_str().map(PathBuf::from)
            } else {
                None
            }
        })
        .context("windows_remote_binary_required")?;
    ensure!(
        binary_platform(&path)? == string(host, "platform")?,
        "remote_binary_platform_mismatch"
    );
    Ok(path)
}
async fn scp(host: &Value, source: &Path, destination: &str) -> Result<()> {
    ensure!(
        !destination.contains(['\0', '\r', '\n']),
        "remote_path_invalid"
    );
    let destination = if host["platform"] == "windows" {
        destination.replace('\\', "/")
    } else {
        destination.to_owned()
    };
    let mut command = Command::new("scp");
    command
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=5",
            "--",
        ])
        .arg(source)
        .arg(format!("{}:{destination}", string(host, "ssh")?));
    native_remote::bounded_process(command, &[], Duration::from_secs(60), 65536, 65536).await?;
    Ok(())
}
pub async fn deploy(c: &Value, names: &[String]) -> Result<()> {
    for name in names {
        let host = &c["hosts"][name];
        ensure!(host.is_object(), "host_unknown");
        if host["local"] == true {
            continue;
        }
        let binary = deployment_binary(c, host)?;
        let helper = string(host, "helper")?;
        let cfg = remote_config(host)?;
        let remote = json!({"codex":host["codex"],"codex_home":host["codex_home"],"verified_version":host["verified_version"]});
        let id = Uuid::new_v4();
        let staging = std::env::temp_dir().join(format!("threadbridge-deploy-{id}.json"));
        atomic(&staging, &serde_json::to_vec(&remote)?)?;
        let result=async{let parent=helper.rsplit_once(if host["platform"]=="windows"{'\\'}else{'/'}).context("helper_parent")?.0;let create=if host["platform"]=="windows"{encoded_ps(&format!("$ErrorActionPreference='Stop';New-Item -ItemType Directory -Force -Path {} | Out-Null",ps_quote(parent)))}else{format!("umask 077; mkdir -p -- {}",shell_quote(parent))};ssh(host,&create,&[],Duration::from_secs(20)).await?;let staged_binary=format!("{helper}.{id}.next");let staged_config=format!("{cfg}.{id}.next");scp(host,&binary,&staged_binary).await?;scp(host,&staging,&staged_config).await?;let apply=if host["platform"]=="windows"{encoded_ps(&format!("$ErrorActionPreference='Stop';Move-Item -Force -LiteralPath {} -Destination {};Move-Item -Force -LiteralPath {} -Destination {}",ps_quote(&staged_binary),ps_quote(helper),ps_quote(&staged_config),ps_quote(&cfg)))}else{format!("chmod 700 -- {b}; chmod 600 -- {c}; mv -f -- {b} {h}; mv -f -- {c} {d}",b=shell_quote(&staged_binary),c=shell_quote(&staged_config),h=shell_quote(helper),d=shell_quote(&cfg))};ssh(host,&apply,&[],Duration::from_secs(20)).await?;let response=rpc(host,&json!({"op":"version"}),Duration::from_secs(12)).await?;println!("{name}: installed; {}; queue={}",string(&response,"stdout")?.trim(),if host["verified_version"].as_str().is_some(){"verified"}else{"read-only"});Ok::<(),anyhow::Error>(())}.await;
        let _ = fs::remove_file(staging);
        result?;
    }
    Ok(())
}
pub async fn run(args: FleetArgs) -> Result<()> {
    let c = load(&args.config)?;
    match args.action {
        FleetAction::Proxy { name, args: extra } => proxy(&c, &name, &args.config, &extra).await?,
        FleetAction::Run { name } => run_host(&c, &name, &args.config).await?,
        FleetAction::Deploy { name } => {
            let names = if let Some(n) = name {
                vec![n]
            } else {
                c["hosts"].as_object().unwrap().keys().cloned().collect()
            };
            deploy(&c, &names).await?;
        }
        FleetAction::Install => install(&c, &args.config).await?,
        FleetAction::Start => {
            systemctl(&["start", "threadbridge.target"]).await?;
        }
        FleetAction::Stop => {
            systemctl(&["stop", "threadbridge.target"]).await?;
        }
        FleetAction::Restart => {
            systemctl(&["restart", "threadbridge.target"]).await?;
        }
        FleetAction::Probe => {
            for (name, host) in c["hosts"].as_object().unwrap() {
                if host["local"] == true {
                    continue;
                }
                match rpc(host, &json!({"op":"version"}), Duration::from_secs(12)).await {
                    Ok(r) => println!("{name}: {}", string(&r, "stdout")?.trim()),
                    Err(_) => println!("{name}: unavailable"),
                }
            }
        }
        FleetAction::Status => {
            for (name, host) in c["hosts"].as_object().unwrap() {
                if host["local"] == true {
                    let mut active = true;
                    for unit in LOCAL_UNITS {
                        if systemctl(&["is-active", "--quiet", unit]).await.is_err() {
                            active = false;
                        }
                    }
                    println!(
                        "{name}: {}",
                        if active {
                            "online; existing local queue"
                        } else {
                            "stopped or partial"
                        }
                    );
                } else {
                    let state = fs::read(status_file(&args.config, name))
                        .ok()
                        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                        .unwrap_or(Value::Null);
                    let fresh = state["online"] == true
                        && native_remote::now() - state["last_success"].as_f64().unwrap_or(0.0)
                            < 15.0;
                    println!(
                        "{name}: {}; {}; {}",
                        if fresh { "online" } else { "offline" },
                        if fresh && state["queue"] == true {
                            "queue verified"
                        } else {
                            "read-only"
                        },
                        state["version"].as_str().unwrap_or("not yet checked")
                    );
                }
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use native_remote::fixtures::Fixture;
    #[test]
    fn snapshot_validation_preserves_old_copy_and_prefixes_title() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let backup = f._temp.path().join("backup.sqlite");
        native_remote::backup(&f.db, &backup).unwrap();
        let destination = f._temp.path().join("phone.sqlite");
        fs::write(&destination, b"previous").unwrap();
        let encode = |v: &[u8]| {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(3));
            z.write_all(v).unwrap();
            STANDARD.encode(z.finish().unwrap())
        };
        assert!(validate_snapshot(&encode(b"invalid"), &destination, "host").is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        validate_snapshot(&encode(&fs::read(backup).unwrap()), &destination, "host").unwrap();
        let title: String = Connection::open(destination)
            .unwrap()
            .query_row("SELECT title FROM captured_replies", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "host · 测试对话");
    }
    #[test]
    fn snapshot_expansion_and_trailing_data_rejected() {
        let f = Fixture::new();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(3));
        z.write_all(&vec![b'a'; DB_BYTES as usize + 1]).unwrap();
        let mut data = z.finish().unwrap();
        assert!(validate_snapshot(
            &STANDARD.encode(&data),
            &f._temp.path().join("large.sqlite"),
            "host"
        )
        .is_err());
        data.extend(b"extra");
        assert!(validate_snapshot(
            &STANDARD.encode(data),
            &f._temp.path().join("bad.sqlite"),
            "host"
        )
        .is_err());
    }
    #[test]
    fn queue_snapshot_excludes_failures_but_keeps_read_replica() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let destination = f._temp.path().join("queue.sqlite");
        queue_snapshot(&f.db, &destination, &[f.native.clone()]).unwrap();
        let db = Connection::open(destination).unwrap();
        for table in TABLES {
            let n: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0);
        }
        assert_eq!(f.count(), 1);
    }
    fn hub(temp: &tempfile::TempDir) -> (Value, Value, PathBuf) {
        let path = temp.path().join("hub.sqlite");
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE devices(id,last_seen);CREATE TABLE capture_targets(host,native,queue_enabled,last_seen);CREATE TABLE threads(host,native,can_send);CREATE TABLE commands(id,status);INSERT INTO devices VALUES('host',10);INSERT INTO capture_targets VALUES('host','bad',1,10),('host','good',1,10);INSERT INTO threads VALUES('host','bad',1),('host','good',1);INSERT INTO commands VALUES('sent','unknown');").unwrap();
        (json!({"hub_db":path}), json!({"host_id":"host"}), path)
    }
    #[test]
    fn quarantine_keeps_other_threads_and_unknown_ledger() {
        let temp = tempfile::tempdir().unwrap();
        let (c, h, path) = hub(&temp);
        quarantine(&c, &h, &["bad".into()]).unwrap();
        let db = Connection::open(path).unwrap();
        assert_eq!(
            db.query_row("SELECT can_send FROM threads WHERE native='bad'", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT can_send FROM threads WHERE native='good'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT status FROM commands", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "unknown"
        );
    }
    #[test]
    fn offline_keeps_immutable_unknown_ledger() {
        let temp = tempfile::tempdir().unwrap();
        let (c, h, path) = hub(&temp);
        offline(&c, &h).unwrap();
        let db = Connection::open(path).unwrap();
        assert_eq!(
            db.query_row("SELECT sum(can_send) FROM threads", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT status FROM commands", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "unknown"
        );
    }
    #[test]
    fn windows_rpc_contains_only_fixed_native_args_and_stdin() {
        let host = json!({"platform":"windows","helper":r"C:\Program Files\ThreadBridge\threadbridge.exe"});
        let cmd = command(&host).unwrap();
        let raw = STANDARD
            .decode(cmd.split_whitespace().last().unwrap())
            .unwrap();
        let utf16 = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        let ps = String::from_utf16(&utf16).unwrap();
        assert!(ps.contains("[Console]::In.ReadToEnd()"));
        assert!(ps.contains(host["helper"].as_str().unwrap()));
        assert!(ps.contains("remote-capture"));
        assert!(!ps.contains("--message"));
        assert!(!ps.contains("python"));
    }
    #[test]
    fn unicode_legal_rpc_is_ascii_bounded_and_roundtrips() {
        let request = json!({"op":"version","text":"é".repeat(16000)+"😀"});
        let raw = ascii_json(&request).unwrap();
        assert!(raw.len() > 65536 && raw.len() < MAX_REQUEST);
        assert!(raw.is_ascii());
        assert_eq!(serde_json::from_slice::<Value>(&raw).unwrap(), request);
    }
    #[test]
    fn proxy_flags_are_message_data_and_freshness_is_required() {
        let id = Uuid::new_v4().to_string();
        let host = json!({"verified_version":"fixture"});
        let state = json!({"online":true,"last_success":native_remote::now(),"blocked_threads":[]});
        let args = vec![
            "queue".into(),
            "--thread".into(),
            id.clone(),
            "--message".into(),
            "--config=evil".into(),
        ];
        assert_eq!(
            proxy_request(&host, &state, &args).unwrap()["text"],
            "--config=evil"
        );
        assert!(proxy_request(&host, &json!({"online":false}), &args).is_err());
        assert!(proxy_request(
            &host,
            &json!({"online":true,"last_success":native_remote::now(),"blocked_threads":[id]}),
            &args
        )
        .is_err());
    }
    #[test]
    fn config_host_count_and_tailnet_targets_checked() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fleet.json");
        let valid = json!({"hosts":{"linux":{"platform":"linux","ssh":"nix@100.10.20.30","helper":"/opt/threadbridge/threadbridge"}}});
        fs::write(&path, valid.to_string()).unwrap();
        assert!(load(&path).is_ok());
        let mut bad = valid.clone();
        bad["hosts"]["linux"]["ssh"] = json!("nix@192.168.0.1");
        fs::write(&path, bad.to_string()).unwrap();
        assert!(load(&path).is_err());
        let mut bad = valid;
        bad["hosts"]["linux"]["ssh"] = json!("nix@100.999.0.1");
        fs::write(&path, bad.to_string()).unwrap();
        assert!(load(&path).is_err());
    }
    #[test]
    fn windows_deploy_requires_native_platform_binary() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("linux");
        fs::write(&binary, b"\x7fELF").unwrap();
        let config = json!({"binary":binary});
        let mut windows = json!({"platform":"windows"});
        assert!(deployment_binary(&config, &windows)
            .unwrap_err()
            .to_string()
            .contains("windows_remote_binary_required"));
        windows["remote_binary"] = json!(binary);
        assert!(deployment_binary(&config, &windows)
            .unwrap_err()
            .to_string()
            .contains("platform_mismatch"));
        let native = temp.path().join("windows.exe");
        fs::write(&native, b"MZ\0\0").unwrap();
        windows["remote_binary"] = json!(native);
        assert!(deployment_binary(&config, &windows).is_ok());
    }
    #[test]
    fn service_exec_uses_native_fleet_and_quotes_config() {
        let text = service_template(
            Path::new("/tmp/space %dir"),
            Path::new("/tmp/threadbridge"),
            Path::new("/tmp/private fleet.json"),
        );
        assert!(text.contains("WorkingDirectory=/tmp/space %%dir\n"));
        assert!(text.contains(
            "ExecStart=\"/tmp/threadbridge\" fleet --config \"/tmp/private fleet.json\" run %i"
        ));
        assert!(!text.contains("python"));
        assert!(text.contains("KillMode=control-group"));
    }
    #[test]
    fn proxy_cli_keeps_message_flags_after_separator() {
        use clap::Parser;
        #[derive(Parser)]
        struct TestCli {
            #[command(flatten)]
            fleet: FleetArgs,
        }
        let id = Uuid::new_v4().to_string();
        let parsed = TestCli::parse_from([
            "fleet",
            "--config",
            "private.json",
            "proxy",
            "windows",
            "--",
            "queue",
            "--thread",
            &id,
            "--message",
            "--config=evil",
        ]);
        assert_eq!(parsed.fleet.config, PathBuf::from("private.json"));
        match parsed.fleet.action {
            FleetAction::Proxy { name, args } => {
                assert_eq!(name, "windows");
                assert_eq!(args.last().unwrap(), "--config=evil");
                assert_eq!(args.len(), 5);
            }
            _ => panic!("wrong action"),
        }
    }
}
