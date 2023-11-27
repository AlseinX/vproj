use std::{collections::HashSet, hash::Hash};

use futures::Future;
use tokio::{
    sync::mpsc::{unbounded_channel, UnboundedSender},
    task::JoinHandle,
};

pub struct Recurse<T, E> {
    join: JoinHandle<Result<(), E>>,
    tx: UnboundedSender<T>,
}

impl<T: Send + 'static, E: Send + 'static> Recurse<T, E> {
    pub fn new<
        U: Clone + Eq + Hash + Send,
        Task: Fn(U, UnboundedSender<T>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        Conv: Fn(T) -> CFut + Send + 'static,
        CFut: Future<Output = Result<U, E>> + Send + 'static,
    >(
        f: Task,
        c: Conv,
    ) -> Self {
        let (tx, mut rx) = unbounded_channel::<T>();
        let mut tx_weak = tx.downgrade();

        Recurse {
            join: tokio::spawn(async move {
                let mut tasks = Vec::new();
                let mut dist = HashSet::new();

                while let Some(t) = rx.recv().await {
                    let a = c(t).await?;
                    if dist.insert(a.clone()) {
                        tasks.push(tokio::spawn(f(
                            a,
                            if let Some(tx) = tx_weak.upgrade() {
                                tx
                            } else {
                                let (tx, rx_new) = unbounded_channel::<T>();
                                tx_weak = tx.downgrade();
                                rx = rx_new;
                                tx
                            },
                        )));
                    }
                }

                for task in tasks {
                    task.await.unwrap()?;
                }

                Ok(())
            }),
            tx,
        }
    }

    pub async fn run(self, v: T) -> Result<(), E> {
        self.tx.send(v).unwrap();
        drop(self.tx);
        self.join.await.unwrap()
    }
}
