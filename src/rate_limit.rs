use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    async_trait,
    extract::{ConnectInfo, FromRef, FromRequestParts},
    http::request::Parts,
};

use crate::state::AppState;

/// Límite de intentos en memoria (ventana deslizante por clave). Alcanza para un solo proceso; con
/// varias instancias habría que moverlo a la base o a un almacén compartido.
#[derive(Default)]
pub struct RateLimiter {
    eventos: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    /// Si la clave ya alcanzó `max` eventos dentro de `ventana`, devuelve cuánto falta para que
    /// pueda volver a intentar.
    pub fn bloqueado(&self, clave: &str, max: usize, ventana: Duration) -> Option<Duration> {
        let mut mapa = self.eventos.lock().unwrap();
        let lista = mapa.get_mut(clave)?;
        let ahora = Instant::now();
        lista.retain(|t| ahora.duration_since(*t) < ventana);
        if lista.len() >= max {
            let mas_viejo = *lista.first()?;
            Some(ventana.saturating_sub(ahora.duration_since(mas_viejo)))
        } else {
            None
        }
    }

    pub fn registrar(&self, clave: &str) {
        let mut mapa = self.eventos.lock().unwrap();
        // Evita que el mapa crezca sin límite ante muchas claves distintas.
        if mapa.len() > 50_000 {
            mapa.clear();
        }
        mapa.entry(clave.to_string())
            .or_default()
            .push(Instant::now());
    }

    pub fn limpiar(&self, clave: &str) {
        self.eventos.lock().unwrap().remove(clave);
    }
}

/// Texto legible para el mensaje de "demasiados intentos".
pub fn espera_legible(d: Duration) -> String {
    let s = d.as_secs().max(1);
    if s < 90 {
        format!("{s} segundos")
    } else {
        format!("{} minutos", (s + 59) / 60)
    }
}

pub struct ClientIp(pub String);

#[async_trait]
impl<S> FromRequestParts<S> for ClientIp
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let estado = AppState::from_ref(state);
        if estado.trust_proxy {
            if let Some(ip) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                return Ok(ClientIp(ip));
            }
        }
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "desconocida".to_string());
        Ok(ClientIp(ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloquea_al_llegar_al_maximo_y_no_antes() {
        let l = RateLimiter::default();
        for _ in 0..4 {
            l.registrar("k");
        }
        assert!(l.bloqueado("k", 5, Duration::from_secs(60)).is_none());
        l.registrar("k");
        assert!(l.bloqueado("k", 5, Duration::from_secs(60)).is_some());
    }

    #[test]
    fn las_claves_son_independientes() {
        let l = RateLimiter::default();
        for _ in 0..5 {
            l.registrar("a");
        }
        assert!(l.bloqueado("a", 5, Duration::from_secs(60)).is_some());
        assert!(l.bloqueado("b", 5, Duration::from_secs(60)).is_none());
    }

    #[test]
    fn limpiar_desbloquea() {
        let l = RateLimiter::default();
        for _ in 0..5 {
            l.registrar("k");
        }
        l.limpiar("k");
        assert!(l.bloqueado("k", 5, Duration::from_secs(60)).is_none());
    }

    #[test]
    fn los_eventos_viejos_salen_de_la_ventana() {
        let l = RateLimiter::default();
        for _ in 0..5 {
            l.registrar("k");
        }
        std::thread::sleep(Duration::from_millis(60));
        assert!(l.bloqueado("k", 5, Duration::from_millis(30)).is_none());
    }

    #[test]
    fn espera_legible_usa_segundos_o_minutos() {
        assert_eq!(espera_legible(Duration::from_secs(20)), "20 segundos");
        assert_eq!(espera_legible(Duration::from_secs(600)), "10 minutos");
    }
}
