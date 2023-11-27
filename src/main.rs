use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Result};

use tasks::Recurse;
use tokio::sync::mpsc::UnboundedSender;
use toml_edit::{Document, Formatted, Item, Value};

mod tasks;

#[tokio::main]
async fn main() -> Result<()> {
    let Some(mut v) = std::env::args().nth(1) else {
        return Err(anyhow!("Must specify a version"));
    };

    if v.starts_with('v') {
        v.remove(0);
    }

    let v = Arc::<str>::from(v);
    Recurse::new(move |target, tx| run(v.clone(), target, tx), calc_target)
        .run(PathBuf::from("."))
        .await?;

    Ok(())
}

async fn calc_target(s: PathBuf) -> Result<PathBuf> {
    let mut path = tokio::fs::canonicalize(s).await?;
    path.push("Cargo.toml");
    Ok(path)
}

async fn run(version: Arc<str>, target: PathBuf, tx: UnboundedSender<PathBuf>) -> Result<()> {
    let mut m: Document = tokio::fs::read_to_string(&target).await?.parse()?;
    modify_dependency(&version, m.get_mut("dependencies"), &target, &tx);
    modify_dependency(&version, m.get_mut("dev-dependencies"), &target, &tx);
    modify_dependency(
        &version,
        m.get_mut("workspace")
            .and_then(|x| x.get_mut("dependencies")),
        &target,
        &tx,
    );

    modify_package(&version, m.get_mut("package"));
    modify_package(
        &version,
        m.get_mut("workspace").and_then(|x| x.get_mut("package")),
    );

    tokio::fs::write(target, m.to_string()).await?;

    Ok(())
}
fn version_item(s: &str) -> Item {
    Item::Value(Value::String(Formatted::new(s.to_string())))
}

fn modify_dependency(
    version: &str,
    item: Option<&mut Item>,
    current: &Path,
    tx: &UnboundedSender<PathBuf>,
) {
    let Some(item) = item else {
        return;
    };

    let Some(item) = item.as_table_like_mut() else {
        return;
    };

    for (_, item) in item.iter_mut() {
        let Some(item) = item.as_table_like_mut() else {
            continue;
        };

        if !item.contains_key("version") {
            item.insert("version", version_item(version));
        }

        if let Some(path) = item.get("path") {
            if let Some(path) = path.as_str() {
                let mut target = current.to_path_buf();
                target.pop();
                target.push(path);
                tx.send(target).unwrap();
            }
        }
    }
}

fn modify_package(v: &str, item: Option<&mut Item>) {
    let Some(item) = item else {
        return;
    };

    if let Some(version) = item.get_mut("version") {
        if version.get("workspace").is_none() {
            *version = version_item(v);
        }
    }
}
