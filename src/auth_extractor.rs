use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::state::AppState;

/// Mismo payload que firmaba `authController.js`: `{ id, nombre, rol }` + `exp` de jsonwebtoken.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub id: i32,
    pub nombre: String,
    pub rol: String,
    pub exp: usize,
}

/// Extractor de ruta: `async fn handler(user: CurrentUser, ...)` falla con 401 automáticamente
/// si falta o es inválido el header `Authorization: Bearer <token>` — reemplaza a
/// `authMiddleware` de Express, pero como parte de la firma del handler en vez de un
/// middleware separado, que es el idiom de Axum para esto.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i32,
    pub nombre: String,
    pub rol: String,
}

#[async_trait]
impl<S> FromRequestParts<S> for CurrentUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;

        let claims = decode_token(token, &state.jwt_secret)?;
        Ok(CurrentUser {
            id: claims.id,
            nombre: claims.nombre,
            rol: claims.rol,
        })
    }
}

pub fn decode_token(token: &str, secret: &str) -> Result<Claims, AppError> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::InvalidToken)
}

/// Equivalente a `requireRole(...roles)` de Express.
pub fn require_role(user: &CurrentUser, roles: &[&str]) -> Result<(), AppError> {
    if roles.contains(&user.rol.as_str()) {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "No tenés permisos para realizar esta acción".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    const SECRETO: &str = "un-secreto-de-prueba-bastante-largo-1234567890";

    fn firmar(claims: &Claims, secreto: &str) -> String {
        encode(
            &Header::default(),
            claims,
            &EncodingKey::from_secret(secreto.as_bytes()),
        )
        .unwrap()
    }
    fn claims(rol: &str, exp: usize) -> Claims {
        Claims {
            id: 7,
            nombre: "Ana".into(),
            rol: rol.into(),
            exp,
        }
    }
    fn en_una_hora() -> usize {
        (chrono::Utc::now().timestamp() + 3600) as usize
    }

    #[test]
    fn acepta_un_token_valido() {
        let c = decode_token(&firmar(&claims("cliente", en_una_hora()), SECRETO), SECRETO).unwrap();
        assert_eq!((c.id, c.rol.as_str()), (7, "cliente"));
    }

    #[test]
    fn rechaza_otro_secreto() {
        let t = firmar(&claims("superadmin", en_una_hora()), "otro-secreto");
        assert!(decode_token(&t, SECRETO).is_err());
    }

    #[test]
    fn rechaza_tokens_vencidos() {
        let t = firmar(&claims("cliente", 1_000), SECRETO);
        assert!(decode_token(&t, SECRETO).is_err());
    }

    #[test]
    fn rechaza_basura_y_alg_none() {
        assert!(decode_token("no-es-un-jwt", SECRETO).is_err());
        assert!(decode_token("", SECRETO).is_err());
        let sin_firma = "eyJhbGciOiJub25lIn0.eyJpZCI6MSwibm9tYnJlIjoieCIsInJvbCI6InN1cGVyYWRtaW4iLCJleHAiOjk5OTk5OTk5OTl9.";
        assert!(decode_token(sin_firma, SECRETO).is_err());
    }

    #[test]
    fn require_role_filtra_por_rol() {
        let u = CurrentUser {
            id: 1,
            nombre: "x".into(),
            rol: "admin".into(),
        };
        assert!(require_role(&u, &["admin", "superadmin"]).is_ok());
        assert!(require_role(&u, &["superadmin"]).is_err());
        assert!(require_role(&u, &[]).is_err());
    }
}
