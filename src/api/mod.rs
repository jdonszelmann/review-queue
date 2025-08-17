use std::{pin::Pin, sync::Arc, time::Duration};

use tokio::{
    sync::{Mutex, MutexGuard},
    time::Instant,
};

pub mod bors;
pub mod crater;
pub mod github;
pub mod rfcbot;
pub mod rollup;

pub struct Cache<'a, T: 'a, P = ()> {
    f: Box<dyn 'a + Send + Sync + Fn(P) -> Pin<Box<dyn 'a + Send + Future<Output = T>>>>,
    last_value: Mutex<(Option<Arc<T>>, Instant)>,
    period: Duration,
}

impl<'a, T: 'a, P: Send> Cache<'a, T, P> {
    pub async fn get_with_param(&self, p: P) -> Arc<T> {
        let guard = self.last_value.lock().await;
        if let (Some(v), t) = &*guard {
            if Instant::now().duration_since(*t) > self.period {
                self.reload(guard, p).await
            } else {
                v.clone()
            }
        } else {
            self.reload(guard, p).await
        }
    }

    async fn reload(&self, mut g: MutexGuard<'_, (Option<Arc<T>>, Instant)>, p: P) -> Arc<T> {
        let new_value = Arc::new((self.f)(p).await);
        *g = (Some(new_value.clone()), Instant::now());

        new_value
    }

    pub fn new_with_param<F: Future<Output = T> + Send + 'a>(
        f: impl 'a + Send + Sync + Fn(P) -> F,
        period: Duration,
    ) -> Self {
        Self {
            f: Box::new(move |p| Box::pin(f(p))),
            last_value: Mutex::new((None, Instant::now())),
            period,
        }
    }
}

impl<'a, T: 'a> Cache<'a, T> {
    pub fn new<F: Future<Output = T> + Send + 'a>(
        f: impl 'a + Send + Sync + Fn() -> F,
        period: Duration,
    ) -> Self {
        Self {
            f: Box::new(move |()| Box::pin(f())),
            last_value: Mutex::new((None, Instant::now())),
            period,
        }
    }

    pub async fn get(&self) -> Arc<T> {
        self.get_with_param(()).await
    }
}
