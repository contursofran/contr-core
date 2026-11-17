//! `herdr-contr-feed install`: the backend install (`install --backend` is the same).
//! Plain text for a person, and an exit code (0 done, 1 stopped, 2 bad arguments); not JSON Lines.

use serde_json::Value;
use std::fs;
use std::path::Path;

use herdr_core::places::{self, Places};

mod backend;
mod files;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}
pub use backend::{KIT, MARKETPLACE};

use backend::{read_remote, Backend};
use files::{put, stop, walk_files};

const HELP: &str = "\
Installs the core of herdr-contr.
Usage: herdr-contr-feed install [--backend] [--home <herdr-app home>] [--helper <herdr-contr-feed>]
                                [--orch <herdr-contr-orch>] [--remote <folder>] [--dry-run]
 - It writes into the core's root only: kit/ (a file there that this version does not have is removed),
   .claude-plugin/marketplace.json, bin/herdr-contr-feed (with --helper), bin/herdr-contr-orch (with --orch),
   remote/ (with --remote), and \"build\" and \"release\" (the version, null for a build from source) in install.json (the record of an install of the mod stays).
 - The root is <home>/herdr-contr with --home, else $CONTR_HOME, else $HERDR_APP_HOME/herdr-contr, else
   the root this helper is installed in, else ~/.config/contr-core when it exists, else ~/.config/contr-backend when it exists, else
   ~/.config/herdr-app/herdr-contr when it exists, else ~/.config/contr-core.
 - --backend is accepted and changes nothing. --dry-run writes nothing.
";

pub fn run(args: &[String]) -> i32 {
    let mut home: Option<String> = None;
    let mut helper: Option<String> = None;
    let mut orch: Option<String> = None;
    let mut remote: Option<String> = None;
    let mut dry = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let value = |what: &str| -> Result<String, i32> {
            args.get(i + 1).cloned().ok_or_else(|| {
                eprintln!("{a} needs {what}");
                2
            })
        };
        match a {
            "--home" => match value("a folder") {
                Ok(v) => home = Some(v),
                Err(c) => return c,
            },
            "--helper" => match value("a file") {
                Ok(v) => helper = Some(v),
                Err(c) => return c,
            },
            "--orch" => match value("a file") {
                Ok(v) => orch = Some(v),
                Err(c) => return c,
            },
            "--remote" => match value("a folder") {
                Ok(v) => remote = Some(v),
                Err(c) => return c,
            },
            "--dry-run" => {
                dry = true;
                i += 1;
                continue;
            }
            "--backend" => {
                i += 1;
                continue;
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return 0;
            }
            _ => {
                eprintln!("unknown argument: {a} (see --help)");
                return 2;
            }
        }
        i += 2;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let Some(root) = places::root(home.as_deref(), None, &env) else {
        return stop(&["STOP: no root of the core is known: CONTR_HOME, HERDR_APP_HOME and HOME are not set. Name the folder with CONTR_HOME=<folder> or --home <herdr-app home>. Nothing was written.".into()]);
    };
    install_backend(&Places::at(root), helper.as_deref(), orch.as_deref(), remote.as_deref(), dry)
}

/// `install --backend`: the kit, the marketplace file, the programs given and `remote/`, into the backend root
/// only, and `"build"` and `"release"` in its install.json (an existing `"files"` stays). No mod file and no `herdr-app check`.
fn install_backend(places: &Places, helper: Option<&str>, orch: Option<&str>, remote: Option<&str>, dry: bool) -> i32 {
    let root = places.folder();
    if let Some(h) = helper {
        if !Path::new(h).is_file() {
            return stop(&[format!("STOP: no helper at {h}")]);
        }
    }
    if let Some(o) = orch {
        if !Path::new(o).is_file() {
            return stop(&[format!("STOP: no orchestration tool at {o}")]);
        }
    }
    let remote = match remote {
        None => None,
        Some(dir) => match read_remote(Path::new(dir)) {
            Ok(r) => Some(r),
            Err(e) => return stop(&[e, "Nothing was written.".into()]),
        },
    };
    let kit_dest = places.kit();
    let mut kit_old = Vec::new();
    walk_files(&kit_dest, &mut kit_old);
    kit_old.retain(|p| !KIT.iter().any(|(rel, _)| kit_dest.join(rel) == *p));
    let backend = Backend { helper, orch, remote, moved: Vec::new(), kit_old };
    println!("{}{}", if dry { "Dry run: would install the core into " } else { "Installing the core into " }, root.display());
    backend.plan(&root, "");
    println!("  install.json (\"build\", \"release\")");
    if dry {
        println!("Dry run: nothing was written.");
        return 0;
    }
    let marker_path = places.install_record();
    let written = (|| -> std::io::Result<()> {
        backend.write(places)?;
        // The record of an install of the mod ("files") stays; only "build" changes.
        let mut record = fs::read(&marker_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        record.insert("build".into(), Value::from(crate::protocol::BUILD));
        record.insert("release".into(), Value::from(crate::update::release()));
        put(&marker_path, (serde_json::to_string_pretty(&record).unwrap() + "\n").as_bytes(), false)
    })();
    if let Err(e) = written {
        return stop(&[format!("STOP: writing into {} failed: {e}", root.display())]);
    }
    // The phone page runs this helper as a service: restart it, or it keeps the old one.
    if helper.is_some() {
        let bin = places.feed();
        let path = bin.canonicalize().unwrap_or(bin);
        if let Some(m) = crate::phone::restart_after_install(&crate::phone::Env::from_process(), &path) {
            println!("{m}");
        }
    }
    println!("Done.");
    0
}
