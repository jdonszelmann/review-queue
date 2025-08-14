use std::{pin::Pin, sync::Arc, time::Duration};

use tokio::{
    sync::{Mutex, MutexGuard},
    time::Instant,
};

pub mod bors;
pub mod crater;
pub mod github;
pub mod rfcbot;

pub struct Cache<'a, T: 'a> {
    f: Box<dyn 'a + Send + Sync + Fn() -> Pin<Box<dyn 'a + Send + Sync + Future<Output = T>>>>,
    last_value: Mutex<(Option<Arc<T>>, Instant)>,
    period: Duration,
}

impl<'a, T: 'a> Cache<'a, T> {
    pub fn new<F: Future<Output = T> + Sync + Send + 'a>(
        f: impl 'a + Send + Sync + Fn() -> F,
        period: Duration,
    ) -> Self {
        Self {
            f: Box::new(move || Box::pin(f())),
            last_value: Mutex::new((None, Instant::now())),
            period,
        }
    }

    async fn reload(&self, mut g: MutexGuard<'_, (Option<Arc<T>>, Instant)>) -> Arc<T> {
        let new_value = Arc::new((self.f)().await);
        *g = (Some(new_value.clone()), Instant::now());

        new_value
    }

    pub async fn get(&self) -> Arc<T> {
        let guard = self.last_value.lock().await;
        if let (Some(v), t) = &*guard {
            if Instant::now().duration_since(*t) > self.period {
                self.reload(guard).await
            } else {
                v.clone()
            }
        } else {
            self.reload(guard).await
        }
    }
}
