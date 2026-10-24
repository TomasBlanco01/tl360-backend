use std::sync::Arc;

use sqlx::PgPool;

use crate::mailer::Mailer;
use crate::push::PushSender;
use crate::rate_limit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: String,
    pub frontend_url: String,
    pub mailer: Mailer,
    pub push: PushSender,
    pub limiter: Arc<RateLimiter>,
    /// Detrás de un proxy (Render) la IP real viene en `X-Forwarded-For`. Solo se confía en ese
    /// header si se activa `TRUST_PROXY=true`; si no, cualquiera podría falsearlo para esquivar
    /// los límites de intentos.
    pub trust_proxy: bool,
}

impl AppState {
    pub fn new(db: PgPool, jwt_secret: String, frontend_url: String) -> Self {
        Self {
            db,
            jwt_secret,
            frontend_url,
            mailer: Mailer::consola(),
            push: PushSender::desactivado(),
            limiter: Arc::new(RateLimiter::default()),
            trust_proxy: false,
        }
    }
}
