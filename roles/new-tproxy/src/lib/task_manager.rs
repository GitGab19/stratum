use std::sync::Mutex as StdMutex;
use tokio::task::JoinHandle;

pub struct TaskManager {
    tasks: StdMutex<Vec<JoinHandle<()>>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: StdMutex::new(Vec::new()),
        } 
    }

    pub fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(async move {
            fut.await;
        });

        self.tasks.lock().unwrap().push(handle);
    }

    pub async fn join_all(&self) {
        let mut tasks = self.tasks.lock().unwrap();
        while let Some(handle) = tasks.pop() {
            let _ = handle.await;
        }
    }

    pub async fn abort_all(&self) {
        let mut tasks = self.tasks.lock().unwrap();
        for handle in tasks.drain(..) {
            handle.abort();
        }
    }
}
