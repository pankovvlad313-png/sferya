//! Maintainer-only integration helper. Not bundled with the application.
mod models;
mod storage;
mod upgrade;

fn run() -> Result<(),String> {
    let args:Vec<_>=std::env::args_os().skip(1).collect();
    if args.len()!=2 { return Err("Usage: sfery-upgrade-probe <explicit-data-dir> <report-file>".into()); }
    let dir=std::path::PathBuf::from(&args[0]);
    let (_guard,store)=upgrade::open_store(&dir,env!("CARGO_PKG_VERSION"))?;
    let snapshot=store.snapshot()?;
    let counts:serde_json::Map<String,serde_json::Value>=["areas","projects","tasks","trash"].into_iter().map(|key|(key.into(),serde_json::json!(snapshot[key].as_array().map_or(0,Vec::len)))).collect();
    let report=serde_json::json!({"passed":true,"version":env!("CARGO_PKG_VERSION"),"dataPath":dir.join("planner.sqlite"),"theme":store.setting("theme")?,"counts":counts});
    std::fs::write(&args[1],serde_json::to_vec_pretty(&report).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    println!("Storage initialization and snapshot completed successfully");
    Ok(())
}
fn main(){if let Err(error)=run(){eprintln!("{error}");std::process::exit(1)}}
