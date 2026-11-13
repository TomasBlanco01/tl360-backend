use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Todas las respuestas de error se devuelven como `{"error": "..."}`, que es una de las dos
/// claves que ya sabe leer `tl360-frontend/src/lib/api.ts` (`body.message ?? body.error`).
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("Token requerido")]
    Unauthorized,
    #[error("Token inválido o expirado")]
    InvalidToken,
    #[error("{0}")]
    AuthFailed(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Hash(#[from] bcrypt::BcryptError),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::InvalidToken => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::AuthFailed(m) => (StatusCode::UNAUTHORIZED, m.clone()),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m.clone()),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            AppError::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, m.clone()),
            AppError::Db(e) => {
                tracing::error!("db error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Error interno".to_string(),
                )
            }
            AppError::Hash(e) => {
                tracing::error!("hash error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Error interno".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Mensaje único de los 403: habla de permisos, no de quién es dueño de qué.
pub const SIN_PERMISOS: &str = "No tenés los permisos para realizar esta acción";

impl AppError {
    /// 403 genérico (recurso ajeno o acción reservada al superadmin).
    pub fn sin_permisos() -> Self {
        AppError::Forbidden(SIN_PERMISOS.into())
    }

    /// 403 de una regla de rol, con el motivo para que la persona sepa qué hacer.
    pub fn sin_permisos_por(motivo: &str) -> Self {
        AppError::Forbidden(format!("{SIN_PERMISOS}: {motivo}"))
    }
}

impl From<axum::extract::rejection::JsonRejection> for AppError {
    fn from(r: axum::extract::rejection::JsonRejection) -> Self {
        AppError::BadRequest(format!("JSON inválido: {}", r.body_text()))
    }
}

/// Igual que `axum::Json` al leer el cuerpo, pero un JSON mal formado responde `{"error": ...}`
/// (que es lo que sabe leer el front) en vez de texto plano.
#[derive(axum::extract::FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub struct AppJson<T>(pub T);
