use std::{collections::HashSet, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Result};
use cargo_toml::{
    Dependency, DependencyDetail, Inheritable, Manifest, Package, PackageTemplate, Workspace,
};
use tokio::{sync::RwLock, task::JoinHandle};

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
                    tasks.push(tokio::spawn(self.clone().run(target)));
                }
                return Ok(());
            }
        }
        let this = self.clone();
        tasks.push(tokio::spawn(async move {
            let mut tasks = this.tasks.write().await;
            if tasks.insert(target.clone()) {
                drop(tasks);
                return this.run(target).await;
            }
            Ok(())
        }));
        Ok(())
    }

    async fn run(self: Arc<Self>, target: PathBuf) -> Result<()> {
        let mut m: Manifest = toml::from_str(&tokio::fs::read_to_string(&target).await?)?;

        let mut tasks = Vec::new();

        for deps in [&mut m.dependencies, &mut m.build_dependencies]
            .into_iter()
            .chain(m.workspace.as_mut().map(|x| &mut x.dependencies))
        {
            for (_, dep) in deps {
                if let Dependency::Detailed(DependencyDetail {
                    version: version @ None,
                    ..
                }) = dep
                {
                    *version = Some(self.version.clone())
                }

                if let Dependency::Detailed(DependencyDetail {
                    path: Some(path), ..
                }) = dep
                {
                    self.add(path.as_str().into(), &mut tasks).await?;
                }
            }
        }

        if let Some(Package {
            version: Inheritable::Set(version),
            ..
        }) = &mut m.package
        {
            *version = self.version.clone();
        }

        if let Some(Workspace {
            package:
                Some(PackageTemplate {
                    version: Some(version),
                    ..
                }),
            ..
        }) = &mut m.workspace
        {
            *version = self.version.clone();
        }

        tokio::fs::write(target, toml::to_string(&m)?).await?;

        for task in tasks {
            task.await??;
        }

        Ok(())
    }
}
