//! RNF-04: alta/baja de suscripciones Web Push y clave pública VAPID para el navegador.

use axum::{extract::State, http::StatusCode, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct SuscripcionRequest {
    endpoint: String,
    keys: ClavesSuscripcion,
}

#[derive(Deserialize)]
pub struct ClavesSuscripcion {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
pub struct BajaRequest {
    endpoint: String,
}

/// Clave pública VAPID (base64url) que necesita `pushManager.subscribe`.
pub async fn clave_publica(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let clave = state.push.clave_publica().ok_or_else(|| {
        AppError::NotFound("Las notificaciones push no están configuradas en el servidor".into())
    })?;
    Ok(Json(json!({ "clave_publica": clave })))
}

fn validar(body: &SuscripcionRequest) -> AppResult<()> {
    let endpoint = body.endpoint.trim();
    if endpoint.len() < 20 || endpoint.len() > 2048 {
        return Err(AppError::BadRequest("Endpoint push inválido".into()));
    }
    let url = reqwest::Url::parse(endpoint)
        .map_err(|_| AppError::BadRequest("Endpoint push inválido".into()))?;
    if !crate::push::endpoint_permitido(&url) {
        return Err(AppError::BadRequest(
            "El endpoint push no pertenece a un servicio soportado".into(),
        ));
    }
    let p256dh = URL_SAFE_NO_PAD
        .decode(body.keys.p256dh.trim())
        .map_err(|_| AppError::BadRequest("Clave p256dh inválida".into()))?;
    if p256dh.len() != 65 || p256dh[0] != 4 {
        return Err(AppError::BadRequest("Clave p256dh inválida".into()));
    }
    let auth = URL_SAFE_NO_PAD
        .decode(body.keys.auth.trim())
        .map_err(|_| AppError::BadRequest("Secreto auth inválido".into()))?;
    if auth.len() != 16 {
        return Err(AppError::BadRequest("Secreto auth inválido".into()));
    }
    Ok(())
}

pub async fn suscribir(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<SuscripcionRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    validar(&body)?;
    let id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO push_suscripciones (usuario_id, endpoint, p256dh, auth) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (endpoint) DO UPDATE SET \
           usuario_id = EXCLUDED.usuario_id, p256dh = EXCLUDED.p256dh, \
           auth = EXCLUDED.auth, creado_en = now() \
         RETURNING id",
    )
    .bind(user.id)
    .bind(body.endpoint.trim())
    .bind(body.keys.p256dh.trim())
    .bind(body.keys.auth.trim())
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

/// Baja idempotente: el navegador puede haber vencido la suscripción antes.
pub async fn desuscribir(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<BajaRequest>,
) -> AppResult<StatusCode> {
    sqlx::query("DELETE FROM push_suscripciones WHERE usuario_id = $1 AND endpoint = $2")
        .bind(user.id)
        .bind(body.endpoint.trim())
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::push::endpoint_permitido;

    #[test]
    fn solo_acepta_endpoints_https_de_servicios_push_conocidos() {
        for bueno in [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://web.push.apple.com/QGx",
            "https://wns2-par02p.notify.windows.com/w/?token=abc",
        ] {
            assert!(endpoint_permitido(&reqwest::Url::parse(bueno).unwrap()));
        }
        for malo in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1/push",
            "https://169.254.169.254/latest/meta-data",
            "https://evil.com/push",
        ] {
            assert!(!endpoint_permitido(&reqwest::Url::parse(malo).unwrap()));
        }
    }
}
