use std::{sync::Mutex, time::Instant};

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use platform_http::Problem;

use crate::state::AppState;

/// Process-wide fixed-window limiter for administrative/registration routes.
pub struct RateLimiter {
    limit: u32,
    window: Mutex<(Instant, u32)>,
}

impl RateLimiter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            limit: per_minute,
            window: Mutex::new((Instant::now(), 0)),
        }
    }

    pub fn allow(&self) -> bool {
        let mut w = self.window.lock().unwrap();
        if w.0.elapsed().as_secs() >= 60 {
            *w = (Instant::now(), 0);
        }
        if w.1 >= self.limit {
            return false;
        }
        w.1 += 1;
        true
    }
}

pub async fn limit_admin(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if st.limiter.allow() {
        next.run(req).await
    } else {
        let mut p = Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests",
        );
        p.retry_after_secs = Some(10);
        p.into_response()
    }
}
