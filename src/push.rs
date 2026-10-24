//! Web Push (RNF-04): VAPID (RFC 8292) + cifrado aes128gcm (RFC 8188/8291) con RustCrypto.
//!
//! Se evita `web-push` porque su dependencia `ece` exige OpenSSL nativo. Acá todo es Rust puro:
//! `p256` para ECDH/ECDSA, `hkdf`/`sha2` para las claves y `aes-gcm` para el payload.
//! Si falta `VAPID_PRIVATE_KEY`, el módulo queda apagado y no afecta al resto de la API.

use std::sync::Arc;
use std::time::Duration;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes128Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use hkdf::Hkdf;
use p256::{
    ecdh::EphemeralSecret,
    ecdsa::{signature::Signer, SigningKey},
    elliptic_curve::sec1::ToEncodedPoint,
    FieldBytes, PublicKey,
};
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use sqlx::PgPool;

struct ConfigPush {
    privada: [u8; 32],
    publica: String,
    sujeto: String,
}

#[derive(Clone)]
pub struct PushSender {
    cliente: reqwest::Client,
    config: Option<Arc<ConfigPush>>,
}

pub struct Notificacion<'a> {
    pub titulo: &'a str,
    pub cuerpo: &'a str,
    pub url: &'a str,
}

#[derive(sqlx::FromRow)]
struct Suscripcion {
    id: i32,
    endpoint: String,
    p256dh: String,
    auth: String,
}

#[derive(Debug)]
enum ErrorEnvio {
    EndpointInvalido,
    Otro(String),
}

impl PushSender {
    pub fn desactivado() -> Self {
        Self {
            cliente: cliente_http(),
            config: None,
        }
    }

    pub fn desde_entorno() -> Self {
        let cliente = cliente_http();
        let privada = match std::env::var("VAPID_PRIVATE_KEY") {
            Ok(clave) if !clave.trim().is_empty() => clave.trim().to_string(),
            _ => {
                return Self {
                    cliente,
                    config: None,
                }
            }
        };
        let sujeto = match std::env::var("VAPID_SUBJECT") {
            Ok(s) if s.starts_with("mailto:") || s.starts_with("https://") => s,
            _ => "mailto:no-reply@tl360.local".to_string(),
        };
        match construir_config(&privada, sujeto) {
            Ok(config) => {
                tracing::info!("Web Push configurado");
                Self {
                    cliente,
                    config: Some(config),
                }
            }
            Err(e) => {
                tracing::error!("VAPID_PRIVATE_KEY inválida ({e}); Web Push queda apagado");
                Self {
                    cliente,
                    config: None,
                }
            }
        }
    }

    pub fn configurado(&self) -> bool {
        self.config.is_some()
    }

    pub fn clave_publica(&self) -> Option<&str> {
        self.config.as_deref().map(|c| c.publica.as_str())
    }

    /// Envía a todos los dispositivos suscritos del usuario. Un endpoint vencido (404/410) se borra.
    /// Devuelve cuántos dispositivos recibieron la notificación; 0 indica que no se entregó.
    pub async fn enviar_a_usuario(
        &self,
        db: &PgPool,
        usuario_id: i32,
        n: &Notificacion<'_>,
    ) -> usize {
        if !self.configurado() {
            return 0;
        }
        let subs = sqlx::query_as::<_, Suscripcion>(
            "SELECT id, endpoint, p256dh, auth FROM push_suscripciones WHERE usuario_id = $1",
        )
        .bind(usuario_id)
        .fetch_all(db)
        .await;
        match subs {
            Ok(subs) => self.enviar_lote(db, subs, n).await,
            Err(e) => {
                tracing::warn!("no se pudieron leer las suscripciones push: {e}");
                0
            }
        }
    }

    /// Envía a todos los usuarios de un rol (por ejemplo, los superadmin al entrar una solicitud).
    pub async fn enviar_a_rol(&self, db: &PgPool, rol: &str, n: &Notificacion<'_>) {
        if !self.configurado() {
            return;
        }
        let subs = sqlx::query_as::<_, Suscripcion>(
            "SELECT p.id, p.endpoint, p.p256dh, p.auth \
             FROM push_suscripciones p JOIN usuarios u ON u.id = p.usuario_id \
             WHERE u.rol = $1",
        )
        .bind(rol)
        .fetch_all(db)
        .await;
        match subs {
            Ok(subs) => {
                self.enviar_lote(db, subs, n).await;
            }
            Err(e) => tracing::warn!("no se pudieron leer las suscripciones push por rol: {e}"),
        }
    }

    async fn enviar_lote(
        &self,
        db: &PgPool,
        subs: Vec<Suscripcion>,
        n: &Notificacion<'_>,
    ) -> usize {
        let mut entregados = 0;
        for sub in subs {
            match self.enviar_una(&sub, n).await {
                Ok(()) => entregados += 1,
                Err(ErrorEnvio::EndpointInvalido) => {
                    tracing::info!("push: se elimina un endpoint vencido");
                    let _ = sqlx::query("DELETE FROM push_suscripciones WHERE id = $1")
                        .bind(sub.id)
                        .execute(db)
                        .await;
                }
                Err(ErrorEnvio::Otro(e)) => {
                    tracing::warn!("push: no se pudo enviar a un dispositivo: {e}");
                }
            }
        }
        entregados
    }

    async fn enviar_una(&self, sub: &Suscripcion, n: &Notificacion<'_>) -> Result<(), ErrorEnvio> {
        let config = self
            .config
            .as_deref()
            .ok_or_else(|| ErrorEnvio::Otro("push no configurado".into()))?;
        let endpoint =
            reqwest::Url::parse(&sub.endpoint).map_err(|_| ErrorEnvio::EndpointInvalido)?;
        if !endpoint_permitido(&endpoint) {
            return Err(ErrorEnvio::EndpointInvalido);
        }
        let p256dh = URL_SAFE_NO_PAD
            .decode(&sub.p256dh)
            .map_err(|_| ErrorEnvio::Otro("p256dh guardada inválida".into()))?;
        let auth = URL_SAFE_NO_PAD
            .decode(&sub.auth)
            .map_err(|_| ErrorEnvio::Otro("auth guardado inválido".into()))?;
        let payload = serde_json::json!({
            "title": n.titulo,
            "body": n.cuerpo,
            "url": n.url,
            "icon": "/icons/icon-192.png",
            "badge": "/icons/icon-192.png"
        })
        .to_string();
        let cuerpo = cifrar(payload.as_bytes(), &p256dh, &auth)?;
        let autorizacion = vapid(config, &endpoint)?;

        let respuesta = self
            .cliente
            .post(endpoint)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("TTL", "43200")
            .header("Urgency", "normal")
            .header("Authorization", autorizacion)
            .body(cuerpo)
            .send()
            .await
            .map_err(|e| ErrorEnvio::Otro(format!("HTTP push: {e}")))?;

        if respuesta.status().is_success() {
            return Ok(());
        }
        let codigo = respuesta.status().as_u16();
        let detalle: String = respuesta
            .bytes()
            .await
            .map(|b| String::from_utf8_lossy(&b).chars().take(300).collect())
            .unwrap_or_default();
        if codigo == 404 || codigo == 410 {
            Err(ErrorEnvio::EndpointInvalido)
        } else {
            Err(ErrorEnvio::Otro(format!("push {codigo}: {detalle}")))
        }
    }
}

fn cliente_http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("el cliente HTTP de push no se pudo crear")
}

/// Solo endpoints de servicios push reales: evita SSRF desde una cuenta autenticada.
pub fn endpoint_permitido(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    matches!(host.as_str(), "fcm.googleapis.com" | "web.push.apple.com")
        || host.ends_with(".push.apple.com")
        || host.ends_with(".push.services.mozilla.com")
        || host.ends_with(".notify.windows.com")
}

fn construir_config(privada: &str, sujeto: String) -> Result<Arc<ConfigPush>, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(privada.trim())
        .map_err(|_| "no es base64url".to_string())?;
    if bytes.len() != 32 {
        return Err(format!(
            "la clave privada debe tener 32 bytes, tiene {}",
            bytes.len()
        ));
    }
    let secreto = FieldBytes::from_slice(&bytes);
    let firmante = SigningKey::from_bytes(secreto).map_err(|e| format!("clave P-256: {e}"))?;
    let publica =
        URL_SAFE_NO_PAD.encode(firmante.verifying_key().to_encoded_point(false).as_bytes());
    Ok(Arc::new(ConfigPush {
        privada: bytes.try_into().expect("ya se validó el largo"),
        publica,
        sujeto,
    }))
}

/// Cifra un único registro aes128gcm y arma el cuerpo del POST (salt | rs | idlen | keyid | ciphertext).
fn cifrar(payload: &[u8], p256dh: &[u8], auth: &[u8]) -> Result<Vec<u8>, ErrorEnvio> {
    if payload.len() > 3900 {
        return Err(ErrorEnvio::Otro("payload push demasiado grande".into()));
    }
    if p256dh.len() != 65 || p256dh[0] != 4 {
        return Err(ErrorEnvio::Otro("p256dh inválida".into()));
    }
    if auth.len() != 16 {
        return Err(ErrorEnvio::Otro("auth inválido".into()));
    }
    let suscriptor =
        PublicKey::from_sec1_bytes(p256dh).map_err(|e| ErrorEnvio::Otro(format!("p256dh: {e}")))?;
    let efimera = EphemeralSecret::random(&mut OsRng);
    let efimera_publica = PublicKey::from(&efimera).to_encoded_point(false);
    let compartido = efimera.diffie_hellman(&suscriptor);

    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let (cek, nonce) = derivar_claves(
        &salt,
        auth,
        compartido.raw_secret_bytes().as_slice(),
        p256dh,
        efimera_publica.as_bytes(),
    )?;

    let mut texto = payload.to_vec();
    texto.push(2); // último (y único) registro
    let cifrador =
        Aes128Gcm::new_from_slice(&cek).map_err(|_| ErrorEnvio::Otro("CEK inválida".into()))?;
    let cifrado = cifrador
        .encrypt(Nonce::from_slice(&nonce), texto.as_ref())
        .map_err(|_| ErrorEnvio::Otro("no se pudo cifrar el payload".into()))?;

    let mut cuerpo = Vec::with_capacity(16 + 4 + 1 + 65 + cifrado.len());
    cuerpo.extend_from_slice(&salt);
    cuerpo.extend_from_slice(&4096u32.to_be_bytes());
    cuerpo.push(65);
    cuerpo.extend_from_slice(efimera_publica.as_bytes());
    cuerpo.extend_from_slice(&cifrado);
    Ok(cuerpo)
}

fn derivar_claves(
    salt: &[u8],
    auth: &[u8],
    compartido: &[u8],
    p256dh: &[u8],
    efimera_publica: &[u8],
) -> Result<([u8; 16], [u8; 12]), ErrorEnvio> {
    // RFC 8291: PRK_key = HKDF(auth, shared); IKM = HKDF-Expand(PRK_key, "WebPush: info" || 0 || ua || as)
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(p256dh);
    info.extend_from_slice(efimera_publica);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), compartido)
        .expand(&info, &mut ikm)
        .map_err(|_| ErrorEnvio::Otro("HKDF del secreto ECDH falló".into()))?;

    let hk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| ErrorEnvio::Otro("HKDF de la CEK falló".into()))?;
    hk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| ErrorEnvio::Otro("HKDF del nonce falló".into()))?;
    Ok((cek, nonce))
}

/// JWT ES256 + `Authorization: vapid t=<jwt>, k=<clave pública>`.
fn vapid(config: &ConfigPush, endpoint: &reqwest::Url) -> Result<String, ErrorEnvio> {
    let host = endpoint
        .host_str()
        .ok_or_else(|| ErrorEnvio::Otro("endpoint sin host".into()))?;
    let mut aud = format!("{}://{host}", endpoint.scheme());
    if let Some(puerto) = endpoint.port() {
        aud.push_str(&format!(":{puerto}"));
    }
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "aud": aud,
            "exp": Utc::now().timestamp() + 12 * 60 * 60,
            "sub": config.sujeto
        })
        .to_string(),
    );
    let entrada = format!("{header}.{claims}");
    let firmante = SigningKey::from_bytes(FieldBytes::from_slice(&config.privada))
        .map_err(|e| ErrorEnvio::Otro(format!("clave VAPID: {e}")))?;
    let firma: p256::ecdsa::Signature = firmante.sign(entrada.as_bytes());
    let firma = URL_SAFE_NO_PAD.encode(firma.to_bytes());
    Ok(format!("vapid t={entrada}.{firma}, k={}", config.publica))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Par de prueba de la documentación de web-push (privada -> pública).
    const PRIVADA: &str = "IQ9Ur0ykXoHS9gzfYX0aBjy9lvdrjx_PFUXmie9YRcY";
    const PUBLICA: &str =
        "BMjQIp55pdbU8pfCBKyXcZjlmER_mXt5LqNrN1hrXbdBS5EnhIbMu3Au-RV53iIpztzNXkGI56BFB1udQ8Bq_H4";

    #[test]
    fn deriva_la_clave_publica_que_usa_el_navegador() {
        let config = construir_config(PRIVADA, "mailto:test@tl360.local".into()).unwrap();
        assert_eq!(config.publica, PUBLICA);
    }

    #[test]
    fn una_clave_invalida_no_habilita_el_envio() {
        assert!(construir_config("no-es-base64-valida", "mailto:x@y".into()).is_err());
    }

    #[test]
    fn cifra_y_descifra_un_payload_como_el_navegador() {
        let suscriptora = EphemeralSecret::random(&mut OsRng);
        let p256dh = PublicKey::from(&suscriptora).to_encoded_point(false);
        let auth = [7u8; 16];
        let payload = br#"{"title":"Hola","body":"Turno confirmado"}"#;
        let cuerpo = cifrar(payload, p256dh.as_bytes(), &auth).unwrap();

        let salt = &cuerpo[..16];
        let idlen = cuerpo[20] as usize;
        assert_eq!(idlen, 65);
        let efimera = PublicKey::from_sec1_bytes(&cuerpo[21..21 + idlen]).unwrap();
        let cifrado = &cuerpo[21 + idlen..];
        let compartido = suscriptora.diffie_hellman(&efimera);
        let (cek, nonce) = derivar_claves(
            salt,
            &auth,
            compartido.raw_secret_bytes().as_slice(),
            p256dh.as_bytes(),
            &cuerpo[21..21 + idlen],
        )
        .unwrap();
        let descifrado = Aes128Gcm::new_from_slice(&cek)
            .unwrap()
            .decrypt(Nonce::from_slice(&nonce), cifrado)
            .unwrap();
        assert_eq!(&descifrado[..descifrado.len() - 1], payload);
        assert_eq!(*descifrado.last().unwrap(), 2);
    }

    #[test]
    fn el_jwt_lleva_la_audiencia_y_la_clave_publica() {
        let config = construir_config(PRIVADA, "mailto:test@tl360.local".into()).unwrap();
        let url = reqwest::Url::parse("https://fcm.googleapis.com/fcm/send/abc").unwrap();
        let valor = vapid(&config, &url).unwrap();
        assert!(valor.starts_with("vapid t="));
        assert!(valor.contains(&format!(", k={PUBLICA}")));
    }

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
