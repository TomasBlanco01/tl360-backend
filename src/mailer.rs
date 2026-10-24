//! Envío de correos transaccionales.
//!
//! Con `SMTP_HOST` configurado sale por SMTP (STARTTLS por defecto, pensado para Brevo/Resend/Gmail);
//! sin esa variable el backend sigue funcionando en desarrollo y escribe el correo en el log, igual
//! que antes. Nunca se falla una operación de negocio porque el correo no salga: se registra el error.

use lettre::{
    message::{Mailbox, Message},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
};

#[derive(Clone)]
pub struct Mailer {
    transporte: Option<AsyncSmtpTransport<Tokio1Executor>>,
    remitente: Mailbox,
}

impl Mailer {
    /// Sin SMTP configurado: los correos se escriben en el log (modo desarrollo/pruebas).
    pub fn consola() -> Self {
        Self {
            transporte: None,
            remitente: remitente_por_defecto(),
        }
    }

    /// Lee `SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM` y `SMTP_TLS`
    /// (`starttls` por defecto, `wrapper` para puerto 465, `none` solo para servidores locales).
    pub fn desde_entorno() -> Self {
        let remitente = std::env::var("SMTP_FROM")
            .ok()
            .and_then(|valor| valor.parse::<Mailbox>().ok())
            .unwrap_or_else(remitente_por_defecto);

        let host = match std::env::var("SMTP_HOST") {
            Ok(host) if !host.trim().is_empty() => host.trim().to_string(),
            _ => return Self::consola_con(remitente),
        };
        let puerto = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(587);
        let modo_tls = std::env::var("SMTP_TLS").unwrap_or_else(|_| "starttls".into());
        let modo_tls = modo_tls.trim().to_lowercase();

        let tls = match modo_tls.as_str() {
            "none" | "insecure" => {
                tracing::warn!("SMTP_TLS=none: el correo y las credenciales viajan sin cifrar");
                Tls::None
            }
            "tls" | "wrapper" | "ssl" => match TlsParameters::new(host.clone()) {
                Ok(params) => Tls::Wrapper(params),
                Err(e) => {
                    tracing::error!("SMTP_TLS inválido para {host}: {e}; se usa el log");
                    return Self::consola_con(remitente);
                }
            },
            _ => match TlsParameters::new(host.clone()) {
                Ok(params) => Tls::Required(params),
                Err(e) => {
                    tracing::error!("no se pudo preparar TLS para {host}: {e}; se usa el log");
                    return Self::consola_con(remitente);
                }
            },
        };

        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(puerto)
            .timeout(Some(std::time::Duration::from_secs(10)))
            .tls(tls);
        let usuario = std::env::var("SMTP_USERNAME").unwrap_or_default();
        let clave = std::env::var("SMTP_PASSWORD").unwrap_or_default();
        if !usuario.trim().is_empty() && !clave.is_empty() {
            builder = builder.credentials(Credentials::new(usuario, clave));
        }

        tracing::info!("SMTP configurado en el puerto {puerto} ({modo_tls})");
        Self {
            transporte: Some(builder.build()),
            remitente,
        }
    }

    pub fn configurado(&self) -> bool {
        self.transporte.is_some()
    }

    /// Envía el correo o, en modo consola, lo deja en el log. El cuerpo puede contener el token:
    /// por eso nunca se loguea cuando SMTP está configurado.
    pub async fn enviar(&self, para: &str, asunto: &str, cuerpo: String) -> Result<(), String> {
        let destino = para
            .trim()
            .parse::<Mailbox>()
            .map_err(|e| format!("destinatario inválido: {e}"))?;
        let Some(transporte) = &self.transporte else {
            tracing::info!("[simulado] correo para {para} — {asunto}\n{cuerpo}");
            return Ok(());
        };

        let email = Message::builder()
            .from(self.remitente.clone())
            .to(destino)
            .subject(asunto)
            .body(cuerpo)
            .map_err(|e| format!("no se pudo armar el correo: {e}"))?;
        transporte
            .send(email)
            .await
            .map(|_| ())
            .map_err(|e| format!("SMTP rechazó el correo: {e}"))
    }

    pub async fn enviar_o_loggear(&self, para: &str, asunto: &str, cuerpo: String) {
        if let Err(e) = self.enviar(para, asunto, cuerpo).await {
            tracing::error!("no se pudo enviar el correo a {para}: {e}");
        }
    }

    fn consola_con(remitente: Mailbox) -> Self {
        Self {
            transporte: None,
            remitente,
        }
    }
}

fn remitente_por_defecto() -> Mailbox {
    "TL360 <no-reply@tl360.local>"
        .parse()
        .expect("el remitente por defecto es válido")
}

pub fn cuerpo_verificacion(nombre: &str, enlace: &str) -> String {
    format!(
        "Hola {nombre}:\n\n\
         Gracias por registrarte en TL360. Confirmá tu cuenta abriendo este enlace:\n\
         {enlace}\n\n\
         El enlace vence en 1 hora y solo se puede usar una vez. Si no creaste la cuenta, ignorá este mensaje.\n\n\
         TL360 - Directorio comercial y turnos de Trenque Lauquen"
    )
}

pub fn cuerpo_reset(nombre: &str, enlace: &str) -> String {
    format!(
        "Hola {nombre}:\n\n\
         Recibimos un pedido para cambiar tu contraseña de TL360. Elegí una nueva acá:\n\
         {enlace}\n\n\
         El enlace vence en 1 hora y solo se puede usar una vez. Si no lo pediste, ignorá este mensaje.\n\n\
         TL360 - Directorio comercial y turnos de Trenque Lauquen"
    )
}

pub fn cuerpo_turno_confirmado(
    nombre: &str,
    comercio: &str,
    fecha: &str,
    hora: &str,
    direccion: &str,
) -> String {
    format!(
        "Hola {nombre}:\n\n\
         Tu turno en {comercio} quedó confirmado para el {fecha} a las {hora}.\n\
         Dirección: {direccion}\n\n\
         Si no podés asistir, cancelalo desde TL360 con la anticipación que pide el comercio.\n\n\
         TL360 - Directorio comercial y turnos de Trenque Lauquen"
    )
}

pub fn cuerpo_turno_cancelado(nombre: &str, comercio: &str, fecha: &str, hora: &str) -> String {
    format!(
        "Hola {nombre}:\n\n\
         Tu turno en {comercio} del {fecha} a las {hora} fue cancelado y el horario quedó libre.\n\n\
         Podés reservar otro turno desde TL360.\n\n\
         TL360 - Directorio comercial y turnos de Trenque Lauquen"
    )
}

/// RF-05: aviso al cliente cuando es el comercio el que cancela el turno.
pub fn cuerpo_turno_cancelado_por_comercio(
    nombre: &str,
    comercio: &str,
    fecha: &str,
    hora: &str,
) -> String {
    format!(
        "Hola {nombre}:\n\n\
         {comercio} canceló tu turno del {fecha} a las {hora} y el horario quedó libre.\n\n\
         Podés reservar otro turno desde TL360.\n\n\
         TL360 - Directorio comercial y turnos de Trenque Lauquen"
    )
}

pub fn cuerpo_verificacion_comercio(nombre: &str, comercio: &str, aprobado: bool) -> String {
    let resolucion = if aprobado {
        "fue aprobada y tu ficha ya muestra el sello de comercio verificado"
    } else {
        "fue rechazada. Revisá los datos de CUIT y la constancia, y volvé a solicitarla desde tu panel"
    };
    format!(
        "Hola {nombre}:\n\n\
         La solicitud de verificación de {comercio} {resolucion}.\n\n\
         TL360 - Directorio comercial y turnos de Trenque Lauquen"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_smtp_el_mailer_queda_en_modo_consola() {
        // No depende de variables del entorno: este constructor nunca arma transporte.
        assert!(!Mailer::consola().configurado());
    }

    #[test]
    fn los_cuerpos_incluyen_el_enlace_y_el_vencimiento() {
        let verificacion =
            cuerpo_verificacion("Ana", "https://tl360.test/auth/verificar?token=abc");
        assert!(verificacion.contains("Ana"));
        assert!(verificacion.contains("token=abc"));
        assert!(verificacion.contains("1 hora"));
        let reset = cuerpo_reset("Ana", "https://tl360.test/auth/reset?token=abc");
        assert!(reset.contains("1 hora"));
    }

    #[test]
    fn el_aviso_de_cancelacion_del_comercio_nombra_al_comercio() {
        let cuerpo =
            cuerpo_turno_cancelado_por_comercio("Ana", "Peluquería Central", "13/10/2026", "10:00");
        assert!(cuerpo.contains("Hola Ana"));
        assert!(cuerpo.contains("Peluquería Central canceló tu turno"));
        assert!(cuerpo.contains("13/10/2026"));
        assert!(cuerpo.contains("10:00"));
        assert!(cuerpo.contains("horario quedó libre"));
    }
}
