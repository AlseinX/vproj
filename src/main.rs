use std::{
    collections::HashSet,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use anyhow::{anyhow, Result};

use tokio::{sync::RwLock, task::JoinHandle};
use toml_edit::{Document, Formatted, Item, Value};

#[tokio::main]
async fn main() -> Result<()> {
    let Some(mut v) = std::env::args().nth(1) else {
        return Err(anyhow!("Must specify a version"));
    };

    if v.starts_with('v') {
        v.remove(0);
    }

    let mut tasks = Vec::new();
    Lookup::new(v).add(".", &mut tasks).await?;
    for task in tasks {
        task.await??;
    }

    Ok(())
}

struct Lookup {
    version: String,
    tasks: RwLock<HashSet<PathBuf>>,
}

impl Lookup {
    fn new(version: String) -> Arc<Self> {
        Arc::new(Self {
            version,
            tasks: RwLock::new(HashSet::new()),
        })
    }

    async fn add(
        self: &Arc<Self>,
        target: &str,
        tasks: &mut Vec<JoinHandle<Result<()>>>,
    ) -> Result<()> {
        let mut abs = tokio::fs::canonicalize(target).await?;
        abs.push("Cargo.toml");
        self.do_add(abs, tasks)
    }

    fn do_add(
        self: &Arc<Self>,
        target: PathBuf,
        tasks: &mut Vec<JoinHandle<Result<()>>>,
    ) -> Result<()> {
        if let Ok(seen_tasks) = self.tasks.try_read() {
            if seen_tasks.contains(&target) {
                return Ok(());
            }
            drop(seen_tasks);
            if let Ok(mut seen_tasks) = self.tasks.try_write() {
                if seen_tasks.insert(target.clone()) {
                    tasks.push(tokio::spawn(UnsafeSend(self.clone().run(target))));
                }
                return Ok(());
            }
        }
        let this = self.clone();
        tasks.push(tokio::spawn(UnsafeSend(async move {
            let mut tasks = this.tasks.write().await;
            if tasks.insert(target.clone()) {
                drop(tasks);
                return this.run(target).await;
            }
            Ok(())
        })));
        Ok(())
    }

    async fn run(self: Arc<Self>, target: PathBuf) -> Result<()> {
        let mut m: Document = tokio::fs::read_to_string(&target).await?.parse()?;

        let mut tasks = Vec::new();

        self.modify_dependency(m.get_mut("dependencies"), &mut tasks)
            .await?;
        self.modify_dependency(m.get_mut("dev-dependencies"), &mut tasks)
            .await?;
        self.modify_dependency(
            m.get_mut("workspace")
                .and_then(|x| x.get_mut("dependencies")),
            &mut tasks,
        )
        .await?;

        self.modify_package(m.get_mut("package"));
        self.modify_package(m.get_mut("workspace").and_then(|x| x.get_mut("package")));

        tokio::fs::write(target, m.to_string()).await?;

        for task in tasks {
            task.await??;
        }

        Ok(())
    }

    async fn modify_dependency(
        self: &Arc<Self>,
        item: Option<&mut Item>,
        tasks: &mut Vec<JoinHandle<Result<()>>>,
    ) -> Result<()> {
        let Some(item) = item else {
            return Ok(());
        };

        let Some(item) = item.as_table_like_mut() else {
            return Ok(());
        };

        for (_, item) in item.iter_mut() {
            let Some(item) = item.as_table_like_mut() else {
                continue;
            };

            if !item.contains_key("version") {
                item.insert("version", self.version_item());
            }

            if let Some(path) = item.get("path") {
                if let Some(path) = path.as_str() {
                    self.add(path, tasks).await?;
                }
            }
        }

        Ok(())
    }

    fn modify_package(&self, item: Option<&mut Item>) {
        let Some(item) = item else {
            return;
        };

        if let Some(version) = item.get_mut("version") {
            *version = self.version_item();
        }
    }

    #[inline(always)]
    fn version_item(&self) -> Item {
        Item::Value(Value::String(Formatted::new(self.version.clone())))
    }
}

struct UnsafeSend<T>(T);

impl<T: Future> Future for UnsafeSend<T> {
    type Output = T::Output;
    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) }.poll(cx)
    }
}

unsafe impl<T> Send for UnsafeSend<T> {}
